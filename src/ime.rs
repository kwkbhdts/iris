use crate::{
    model::{self, Backend, InputReply, DEADLINE_MS, RESTORE_DELAY_MS},
    types::*,
};
use std::{
    mem::size_of,
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{ProcessStatus::K32GetModuleBaseNameW, SystemInformation::GetTickCount, Threading::*},
    UI::{
        Input::{Ime::ImmGetDefaultIMEWnd, KeyboardAndMouse::*},
        WindowsAndMessaging::*,
    },
};

pub static EPOCH: AtomicU64 = AtomicU64::new(0);
pub static ABORT: AtomicBool = AtomicBool::new(false);
// 無効化・終了は新規入力を止めるが、条件が保たれた復元は妨げない。
pub static STOP: AtomicBool = AtomicBool::new(false);
const QUERY_TIMEOUT_MS: u32 = 50;
const IMC_GETOPENSTATUS: usize = 0x0005;
const IMC_SETOPENSTATUS: usize = 0x0006;

#[derive(Clone, Copy)]
pub struct Request {
    pub snapshot: Snapshot,
    pub epoch: u64,
}
pub struct Completion {
    pub id: usize,
    pub warning: Option<&'static str>,
}

/// F1 時点のウィンドウ識別情報だけを取り、重い処理は作業スレッドへ回す。
pub fn capture(target: WindowId, at: u32) -> Request {
    Request {
        snapshot: Snapshot {
            id: 0,
            at,
            foreground: Ok(target),
            focus: focused(target),
        },
        epoch: EPOCH.load(Ordering::SeqCst),
    }
}

/// 前面の識別情報だけを取得する。
pub fn foreground() -> Option<WindowId> {
    window_id(unsafe { GetForegroundWindow() }).ok()
}

/// 実行ファイルの末尾名だけで今回の対象を確認する。
pub fn is_allowed(target: WindowId) -> bool {
    process_name(target.pid).is_ok_and(|name| crate::logic::allowed_image(&name))
}

/// ウィンドウの存在だけで他の Iris を確認し、タイトルは読まない。
pub fn other_iris_running() -> bool {
    [
        "Iris.Diagnostic.Window",
        "Iris.ImeTrial.Window",
        "Iris.InputTrial.Window",
    ]
    .iter()
    .any(|name| {
        let class: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        unsafe { !FindWindowW(class.as_ptr(), null_mut()).is_null() }
    })
}

/// テキストを取得せず、ウィンドウの所属だけを調べる。
fn window_id(window: HWND) -> Result<WindowId, u32> {
    unsafe {
        SetLastError(0);
        let mut pid = 0;
        let tid = GetWindowThreadProcessId(window, &mut pid);
        if window.is_null() || pid == 0 || tid == 0 {
            Err(GetLastError())
        } else {
            Ok(WindowId {
                hwnd: window as usize,
                pid,
                tid,
            })
        }
    }
}

/// GUI スレッドからフォーカス HWND の識別情報だけを取り出す。
fn focused(foreground: WindowId) -> Result<WindowId, u32> {
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        SetLastError(0);
        if GetGUIThreadInfo(foreground.tid, &mut info) == 0 {
            return Err(GetLastError());
        }
        window_id(info.hwndFocus)
    }
}

/// F1 前後の前面とフォーカスが一致するか確認する。
fn unchanged(snapshot: Snapshot) -> bool {
    let foreground = window_id(unsafe { GetForegroundWindow() });
    foreground == snapshot.foreground && foreground.and_then(focused) == snapshot.focus
}

/// 実行ファイルの末尾名だけを取得する。パスやプロセス内の入力内容は読まない。
fn process_name(pid: u32) -> Result<String, u32> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid);
        if process.is_null() {
            return Err(GetLastError());
        }
        let mut name = [0u16; 260];
        SetLastError(0);
        let len = K32GetModuleBaseNameW(process, null_mut(), name.as_mut_ptr(), name.len() as u32);
        let code = GetLastError();
        CloseHandle(process);
        if len == 0 {
            return Err(code);
        }
        if len as usize >= name.len() {
            return Err(ERROR_INSUFFICIENT_BUFFER);
        }
        String::from_utf16(&name[..len as usize]).map_err(|_| ERROR_NO_UNICODE_TRANSLATION)
    }
}

/// 対象 IME ウィンドウへの取得・設定要求に 50ms の待機上限を置く。
fn message(window: WindowId, command: usize, value: isize) -> ImeReply {
    unsafe {
        // 同一スレッドでは SendMessageTimeout の期限が働かないので問い合わせない。
        if window.tid == GetCurrentThreadId() {
            return ImeReply::NotQueried;
        }
        let mut result = 0;
        SetLastError(0);
        let ok = SendMessageTimeoutW(
            window.hwnd as HWND,
            WM_IME_CONTROL,
            command,
            value,
            SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT,
            QUERY_TIMEOUT_MS,
            &mut result,
        );
        if ok == 0 {
            ImeReply::Failed(GetLastError())
        } else {
            ImeReply::Raw(result)
        }
    }
}

/// 読み取りコマンドにはデータを渡さない。
fn query(window: WindowId, command: usize) -> ImeReply {
    message(window, command, 0)
}

/// 自分の注入イベントに、このプロセス内だけで使う識別値を付ける。
pub fn input_tag() -> usize {
    &EPOCH as *const AtomicU64 as usize
}

/// Unicode または Enter の一イベントを作る。
fn key_input(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: input_tag(),
            },
        },
    }
}

struct ImeBackend {
    snapshot: Snapshot,
    epoch: u64,
    ime: WindowId,
    layout: usize,
}
impl Backend for ImeBackend {
    /// 各操作の直前に、対象・レイアウト・期限・介入を再確認する。
    fn guarded(&mut self) -> bool {
        let quick = || {
            !ABORT.load(Ordering::SeqCst)
                && EPOCH.load(Ordering::SeqCst) == self.epoch
                && model::within_deadline(self.snapshot.at, unsafe { GetTickCount() })
        };
        if !quick()
            || !unchanged(self.snapshot)
            || other_iris_running()
            || [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
                .iter()
                .any(|&vk| unsafe { GetAsyncKeyState(vk as i32) } < 0)
        {
            return false;
        }
        let (Ok(fg), Ok(focus)) = (self.snapshot.foreground, self.snapshot.focus) else {
            return false;
        };
        if !is_allowed(fg) || self.layout == 0 {
            return false;
        }
        let ime = window_id(unsafe { ImmGetDefaultIMEWnd(focus.hwnd as HWND) });
        ime == Ok(self.ime)
            && self.ime.pid == focus.pid
            && self.ime.tid == focus.tid
            && unsafe { GetKeyboardLayout(focus.tid) } as usize == self.layout
            && unchanged(self.snapshot)
            && quick()
    }
    /// 終了要求では新規のオフだけを止め、条件が保たれれば復元する。
    fn closing(&self) -> bool {
        STOP.load(Ordering::SeqCst)
    }
    /// 開閉の応答を取得する。取得不能をオフとみなさない。
    fn read_open(&mut self) -> ImeReply {
        query(self.ime, IMC_GETOPENSTATUS)
    }
    /// 変更対象は保存した IME ウィンドウの開閉だけに限定する。
    fn set_open(&mut self, open: bool) -> ImeReply {
        message(self.ime, IMC_SETOPENSTATUS, isize::from(open))
    }
    /// 一括投入直前にも対象と終了要求を確認する。追加の Enter や再送は行わない。
    fn send_once(&mut self) -> InputReply {
        if self.closing() || !self.guarded() || unsafe { GetAsyncKeyState(VK_RETURN as i32) } < 0 {
            return InputReply {
                attempted: false,
                inserted: 0,
            };
        }
        let inputs = [
            key_input(0, 'O' as u16, KEYEVENTF_UNICODE),
            key_input(0, 'O' as u16, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
            key_input(0, 'K' as u16, KEYEVENTF_UNICODE),
            key_input(0, 'K' as u16, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
            key_input(VK_RETURN, 0, 0),
            key_input(VK_RETURN, 0, KEYEVENTF_KEYUP),
        ];
        unsafe {
            SetLastError(0);
            let inserted = SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                size_of::<INPUT>() as i32,
            );
            InputReply {
                attempted: true,
                inserted,
            }
        }
    }

    /// 投入後の復元用実験待機。文字と Enter の間には待たない。
    fn wait_before_restore(&mut self) {
        let start = unsafe { GetTickCount() };
        while unsafe { GetTickCount() }.wrapping_sub(start) < RESTORE_DELAY_MS {
            if !self.guarded() {
                ABORT.store(true, Ordering::SeqCst);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// 一件の処理を実行し、入力・復元を確認できないときだけ利用者へ通知する。
pub fn execute(request: Request) -> Completion {
    let snapshot = request.snapshot;
    let warning = if STOP.load(Ordering::SeqCst)
        || ABORT.load(Ordering::SeqCst)
        || EPOCH.load(Ordering::SeqCst) != request.epoch
        || !model::within_deadline(snapshot.at, unsafe { GetTickCount() })
        || !unchanged(snapshot)
    {
        None
    } else {
        prepare_and_run(request)
    };
    Completion {
        id: snapshot.id,
        warning,
    }
}

/// IME の所有者とレイアウトを取得し、比較版と同じ手順で入力・復元する。
fn prepare_and_run(request: Request) -> Option<&'static str> {
    let Ok(focus) = request.snapshot.focus else {
        return Some("入力フォーカスを確認できないため送信せず、Iris を無効にしました。");
    };
    let ime = window_id(unsafe { ImmGetDefaultIMEWnd(focus.hwnd as HWND) });
    let Ok(ime) = ime else {
        return Some("IME の状態を取得できないため送信せず、Iris を無効にしました。");
    };
    let layout = unsafe { GetKeyboardLayout(focus.tid) } as usize;
    if layout == 0 || ime.pid != focus.pid || ime.tid != focus.tid {
        return Some(
            "IME の所属またはレイアウトを確認できないため送信せず、Iris を無効にしました。",
        );
    }
    let mut backend = ImeBackend {
        snapshot: request.snapshot,
        epoch: request.epoch,
        ime,
        layout,
    };
    let result = model::run(&mut backend);
    if result.manual {
        Some("入力または IME 復元を確認できないため Iris を無効にしました。\n入力・送信結果、キー状態、元の入力欄の IME を手動確認・復元してください。自動再送はしません。")
    } else if matches!(
        result.status,
        "original_unknown; no_input_or_change" | "off_not_confirmed_or_guard_failed; no_input"
    ) {
        Some("IME オフを確認できないため送信せず、Iris を無効にしました。\n対象の入力欄と IME を確認してください。")
    } else {
        None
    }
}

/// UI 側の期限確認にも同じ基準を使う。
pub fn expired(request: Request) -> bool {
    unsafe { GetTickCount() }.wrapping_sub(request.snapshot.at) > DEADLINE_MS
}
