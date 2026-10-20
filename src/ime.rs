use crate::{
    model::guard::{EpochEvidence, EventKind, Evidence, Reason},
    model::input_history::{Check, History},
    model::{self, Backend, InputReply, DEADLINE_MS},
    types::*,
};
use std::{
    mem::size_of,
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
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
static EPOCH_EVIDENCE: EpochEvidence = EpochEvidence::EMPTY;
static INPUT_HISTORY: History = History::EMPTY;
pub static ABORT: AtomicBool = AtomicBool::new(false);
// 無効化・終了は新規入力を止めるが、条件が保たれた復元は妨げない。
pub static STOP: AtomicBool = AtomicBool::new(false);
const QUERY_TIMEOUT_MS: u32 = 50;
const IMC_GETOPENSTATUS: usize = 0x0005;
const IMC_SETOPENSTATUS: usize = 0x0006;

/// 元の世代加算を維持し、診断用にイベントの分類だけを添える。
pub fn invalidate(kind: EventKind) {
    let epoch = EPOCH.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
    INPUT_HISTORY.record(epoch, matches!(kind, EventKind::PointerAllowed));
    EPOCH_EVIDENCE.record(epoch, kind);
}

#[derive(Clone, Copy)]
pub struct Request {
    pub snapshot: Snapshot,
    pub epoch: u64,
}
pub struct Completion {
    pub id: usize,
    pub warning: Option<crate::warning::Warning>,
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
    unchanged_reason(snapshot).is_ok()
}

/// 同じ取得順・同じ比較条件で、既に取得した不一致だけを分類する。
fn unchanged_reason(snapshot: Snapshot) -> Result<(), Reason> {
    let foreground = window_id(unsafe { GetForegroundWindow() });
    if foreground != snapshot.foreground {
        return Err(if foreground.is_err() || snapshot.foreground.is_err() {
            Reason::ForegroundUnknown
        } else {
            Reason::ForegroundChanged
        });
    }
    let focus = foreground.and_then(focused);
    if focus != snapshot.focus {
        return Err(if focus.is_err() || snapshot.focus.is_err() {
            Reason::FocusUnknown
        } else {
            Reason::FocusChanged
        });
    }
    Ok(())
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

pub struct ImeBackend {
    input_plan: model::InputPlan,
    snapshot: Snapshot,
    epoch: u64,
    ime: WindowId,
    layout: usize,
    evidence: Evidence,
}

impl ImeBackend {
    /// 明示比較の入力列だけを選ぶ。通常の準備ではEnter1回を維持する。
    #[allow(dead_code)]
    pub fn with_input_plan(mut self, plan: model::InputPlan) -> Self {
        self.input_plan = plan;
        self
    }

    /// 後追い観測にも準備時の履歴を渡す。世代を更新しない。
    #[allow(dead_code)]
    pub fn observation_epoch(&self) -> u64 {
        self.epoch
    }

    /// 失敗後の読取り診断へ、変更前の保存済み識別情報だけを渡す。
    #[allow(dead_code)]
    pub fn identity_snapshot(&self) -> (Snapshot, WindowId, usize) {
        (self.snapshot, self.ime, self.layout)
    }

    /// 不成立の理由だけを保存し、判定結果は変えない。
    fn failed(&mut self, reason: Reason) -> bool {
        self.evidence.current = reason;
        false
    }

    /// 元の短絡順でフラグ・世代・期限を一回ずつ読む。
    fn quick(&mut self) -> bool {
        if ABORT.load(Ordering::SeqCst) {
            return self.failed(Reason::Abort);
        }
        let epoch = EPOCH.load(Ordering::SeqCst);
        match INPUT_HISTORY.check(self.epoch, epoch, &EPOCH) {
            Check::Unchanged | Check::AllowedTransition => {}
            Check::PointerOnly => self.evidence.saw_pointer_only = true,
            Check::BlockingEvent => return self.failed(Reason::HistoryBlockingEvent),
            Check::Unknown => return self.failed(Reason::HistoryUnknown),
        }
        if !model::within_deadline(self.snapshot.at, unsafe { GetTickCount() }) {
            return self.failed(Reason::Deadline);
        }
        true
    }
}

impl Backend for ImeBackend {
    /// 各操作の直前に、対象・レイアウト・期限・介入を再確認する。
    fn guarded(&mut self) -> bool {
        self.evidence.current = Reason::Unknown;
        if !self.quick() {
            return false;
        }
        if let Err(reason) = unchanged_reason(self.snapshot) {
            return self.failed(reason);
        }
        if other_iris_running() {
            return self.failed(Reason::OtherIris);
        }
        if crate::logic::win32::modifiers_down() {
            return self.failed(Reason::ModifierDown);
        }
        let (Ok(fg), Ok(focus)) = (self.snapshot.foreground, self.snapshot.focus) else {
            return self.failed(Reason::SnapshotUnknown);
        };
        if !is_allowed(fg) {
            return self.failed(Reason::TargetUnconfirmed);
        }
        if self.layout == 0 {
            return self.failed(Reason::LayoutUnknown);
        }
        let ime = window_id(unsafe { ImmGetDefaultIMEWnd(focus.hwnd as HWND) });
        if ime != Ok(self.ime) {
            return self.failed(Reason::ImeWindowChangedOrUnknown);
        }
        if self.ime.pid != focus.pid || self.ime.tid != focus.tid {
            return self.failed(Reason::ImeOwnerMismatch);
        }
        if unsafe { GetKeyboardLayout(focus.tid) } as usize != self.layout {
            return self.failed(Reason::LayoutChanged);
        }
        if let Err(reason) = unchanged_reason(self.snapshot) {
            return self.failed(reason);
        }
        self.quick()
    }

    /// 待機で先に観測した不成立は、後続の中止フラグと区別して返す。
    fn guard_evidence(&self) -> Evidence {
        self.evidence
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
    /// 一括投入直前に対象と終了を確認し、選択した固定列を一度だけ投入する。
    fn send_once(&mut self) -> InputReply {
        if self.closing() || !self.guarded() || unsafe { GetAsyncKeyState(VK_RETURN as i32) } < 0 {
            return InputReply {
                attempted: false,
                inserted: 0,
            };
        }
        let mut storage = [INPUT::default(); 8];
        let events = self.input_plan.events();
        for (slot, &(key, up)) in storage.iter_mut().zip(events) {
            let release = if up { KEYEVENTF_KEYUP } else { 0 };
            *slot = match key {
                model::InputKey::Unicode(value) => key_input(0, value, KEYEVENTF_UNICODE | release),
                model::InputKey::Enter => key_input(VK_RETURN, 0, release),
            };
        }
        let inputs = &storage[..events.len()];
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
        if !model::wait_before_restore(self) {
            self.evidence.wait = Some(self.evidence.current);
            ABORT.store(true, Ordering::SeqCst);
        }
    }
}

/// 一件の処理を実行し、入力・復元を確認できないときだけ利用者へ通知する。
pub fn execute(request: Request) -> Completion {
    let snapshot = request.snapshot;
    let warning = if STOP.load(Ordering::SeqCst)
        || ABORT.load(Ordering::SeqCst)
        || !INPUT_HISTORY
            .check(request.epoch, EPOCH.load(Ordering::SeqCst), &EPOCH)
            .permits()
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
fn prepare_and_run(request: Request) -> Option<crate::warning::Warning> {
    use crate::warning::Warning;
    let mut backend =
        match prepare_backend(request) {
            Ok(backend) => backend,
            Err(reason) => return Some(match reason {
                "focus_snapshot_failed" => Warning::new(
                    "IRIS-P001",
                    "入力フォーカスを確認できないため送信せず、Iris を無効にしました。",
                ),
                "ime_unknown" => Warning::new(
                    "IRIS-P002",
                    "IME の状態を取得できないため送信せず、Iris を無効にしました。",
                ),
                _ => Warning::new(
                    "IRIS-P003",
                    "IME の所属またはレイアウトを確認できないため送信せず、Iris を無効にしました。",
                ),
            }),
        };
    let result = model::run(&mut backend);
    if result.manual {
        Some(Warning::new(result.diagnostic(), "入力または IME 復元を確認できないため Iris を無効にしました。\n入力・送信結果、キー状態、元の入力欄の IME を手動確認・復元してください。自動再送はしません。").with_restore(result.restore))
    } else if matches!(
        result.status,
        "original_unknown; no_input_or_change" | "off_not_confirmed_or_guard_failed; no_input"
    ) {
        Some(Warning::new(result.diagnostic(), "IME オフを確認できないため送信せず、Iris を無効にしました。\n対象の入力欄と IME を確認してください。"))
    } else {
        None
    }
}

/// 両版で同じ所属・レイアウト確認と移動許容ポリシーを準備する。
fn prepare_backend(request: Request) -> Result<ImeBackend, &'static str> {
    let focus = request
        .snapshot
        .focus
        .map_err(|_| "focus_snapshot_failed")?;
    if request
        .snapshot
        .foreground
        .map_or(true, |target| focus.pid != target.pid)
    {
        return Err("focus_process_changed");
    }
    let ime =
        window_id(unsafe { ImmGetDefaultIMEWnd(focus.hwnd as HWND) }).map_err(|_| "ime_unknown")?;
    let layout = unsafe { GetKeyboardLayout(focus.tid) } as usize;
    if layout == 0 || ime.pid != focus.pid || ime.tid != focus.tid {
        return Err("ime_owner_or_layout_unknown");
    }
    Ok(ImeBackend {
        input_plan: model::InputPlan::SingleEnter,
        snapshot: request.snapshot,
        epoch: request.epoch,
        ime,
        layout,
        evidence: Evidence {
            pointer_allowed: true,
            ..Evidence::UNKNOWN
        },
    })
}

/// オート試作の検証後に新しいWin32フォーカスを取得し、既存バックエンドを準備する。
#[allow(dead_code)]
pub fn prepare_auto(target: WindowId) -> Result<ImeBackend, &'static str> {
    let request = capture(target, unsafe { GetTickCount() });
    let mut backend = prepare_backend(request)?;
    if !backend.guarded() {
        return Err("fresh_snapshot_invalid");
    }
    Ok(backend)
}

/// UI 側の期限確認にも同じ基準を使う。
pub fn expired(request: Request) -> bool {
    unsafe { GetTickCount() }.wrapping_sub(request.snapshot.at) > DEADLINE_MS
}

/// 現在の入力先を読み、復元照合と診断に使う。取得不能はNoneに保つ。
#[allow(dead_code)]
pub fn read_identity() -> (
    Option<WindowId>,
    Option<WindowId>,
    Option<WindowId>,
    Option<usize>,
) {
    let foreground = window_id(unsafe { GetForegroundWindow() }).ok();
    let focus = foreground.and_then(|w| focused(w).ok());
    let ime = focus.and_then(|w| window_id(unsafe { ImmGetDefaultIMEWnd(w.hwnd as HWND) }).ok());
    let layout = focus.and_then(|w| {
        let value = unsafe { GetKeyboardLayout(w.tid) } as usize;
        (value != 0).then_some(value)
    });
    (foreground, focus, ime, layout)
}

/// 保存IME窓のconversionを観測する。生の0も対応保証や英数確定にはしない。
#[allow(dead_code)]
pub fn read_conversion(window: WindowId) -> ImeReply {
    query(window, 0x0001)
}
/// 保存IME窓のopenを後追いで読む。書込みや期限の変更は行わない。
#[allow(dead_code)]
pub fn read_saved_open(window: WindowId) -> ImeReply {
    query(window, IMC_GETOPENSTATUS)
}

/// 書込み期限を更新せず、元の介入履歴と停止要求だけを読取り用に確認する。
#[allow(dead_code)]
pub fn observation_history_valid(epoch: u64) -> bool {
    !ABORT.load(Ordering::SeqCst)
        && !STOP.load(Ordering::SeqCst)
        && INPUT_HISTORY
            .check(epoch, EPOCH.load(Ordering::SeqCst), &EPOCH)
            .permits()
        && !other_iris_running()
}
