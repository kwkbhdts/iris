use std::{
    cell::{Cell, RefCell},
    mem::size_of,
    ptr::null_mut,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{
        LibraryLoader::GetModuleHandleW, ProcessStatus::K32GetModuleBaseNameW,
        SystemInformation::GetTickCount, Threading::*,
    },
    UI::{
        Accessibility::*,
        Controls::EM_SETSEL,
        Input::{Ime::ImmGetDefaultIMEWnd, KeyboardAndMouse::*},
        WindowsAndMessaging::*,
    },
};

use super::{
    diagnostic::*,
    model::{self, Backend, InputReply, DEADLINE_MS, RESTORE_DELAY_MS},
};

const WM_CAPTURE: u32 = WM_APP + 1;
const SELECT: usize = 1;
const POLL: usize = 1;
const QUERY_TIMEOUT_MS: u32 = 50;
// 従来の IMM 互換問い合わせ。対応は保証せず、生の応答だけを記録する。
const IMC_GETCONVERSIONMODE: usize = 0x0001;
const IMC_GETOPENSTATUS: usize = 0x0005;
const IMC_SETOPENSTATUS: usize = 0x0006;
const INTRO: &str = "Iris IME 入力比較（OK と Enter を実際に送ります）\r\n通常版・読み取り診断版・切替試験版は終了してください。\r\n送信してよい試験先の空欄・未確定文字なしでのみ使用。\r\n対象入力欄で単独 F1 を1回。結果までキー・マウスに触れないでください。\r\nIME オフ応答を確認→OK と Enter（間の待機なし）。\r\n元がオンの場合のみ、投入後100ms待機して条件付き復元。\r\nこの100msは復元用の実験値で、アプリの処理完了保証ではありません。\r\n失敗・介入時は入力結果・キー状態・IME を手動確認してください。強制終了時の復元保証なし。\r\n結果を選択→Ctrl+C。起動中は全アプリの単独 F1 を抑止します。\r\n\r\n";
static EPOCH: AtomicU64 = AtomicU64::new(0);
static ABORT: AtomicBool = AtomicBool::new(false);
static CLOSING: AtomicBool = AtomicBool::new(false);
static LOCKED: AtomicBool = AtomicBool::new(false);
static FINISHING: AtomicBool = AtomicBool::new(false);

struct ResultReport {
    snapshot: Snapshot,
    text: String,
    manual: bool,
}
impl ResultReport {
    /// 取得結果と試験の生の応答をまとめて表示する。
    fn render(&self) -> String {
        self.text.clone()
    }
}

#[derive(Clone, Copy)]
struct HookState {
    window: HWND,
    gate: F1Gate,
    pending: Option<Snapshot>,
    next_id: usize,
    pending_epoch: u64,
    timeout_reported: bool,
    close_at: Option<u32>,
}

struct App {
    edit: HWND,
    button: HWND,
    requests: SyncSender<(Snapshot, u64)>,
    replies: Receiver<ResultReport>,
    records: Vec<String>,
}

thread_local! {
    static HOOK: Cell<HookState> = const { Cell::new(HookState {
        window: null_mut(), gate: F1Gate::EMPTY, pending: None, next_id: 1,
        pending_epoch: 0, timeout_reported: false, close_at: None,
    }) };
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Win32 用の終端付き文字列を作る。
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// 診断の開始に失敗した理由を表示する。
fn error(text: &str) {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(text).as_ptr(),
            wide("Iris IME 入力比較").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
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

/// フックは識別情報を採取して予約するだけで、プロセス・IME の照会はしない。
unsafe extern "system" fn keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let mut state = HOOK.get();
    if code != HC_ACTION as i32 || state.window.is_null() {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    let key = &*(lparam as *const KBDLLHOOKSTRUCT);
    if model::own_input(
        key.flags & LLKHF_INJECTED != 0,
        key.dwExtraInfo,
        input_tag(),
    ) {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    if key.vkCode != VK_F1 as u32 || key.flags & LLKHF_INJECTED != 0 {
        EPOCH.fetch_add(1, Ordering::SeqCst);
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    let down = match wparam as u32 {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_KEYUP | WM_SYSKEYUP => false,
        _ => return CallNextHookEx(null_mut(), code, wparam, lparam),
    };
    let modified = key.flags & LLKHF_ALTDOWN != 0
        || [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
            .iter()
            .any(|&vk| GetAsyncKeyState(vk as i32) < 0);
    let (swallow, capture) = state.gate.event(
        down,
        modified,
        state.pending.is_some() || LOCKED.load(Ordering::SeqCst) || CLOSING.load(Ordering::SeqCst),
    );
    if capture {
        if let Some(next) = state.next_id.checked_add(1) {
            let foreground = window_id(GetForegroundWindow());
            let snapshot = Snapshot {
                id: state.next_id,
                at: key.time,
                foreground,
                focus: foreground.and_then(focused),
            };
            state.next_id = next;
            state.pending = Some(snapshot);
            state.pending_epoch = EPOCH.load(Ordering::SeqCst);
            state.timeout_reported = false;
            ABORT.store(false, Ordering::SeqCst);
            if PostMessageW(state.window, WM_CAPTURE, snapshot.id, 0) == 0 {
                state.pending = None;
            }
        }
    }
    HOOK.set(state);
    if swallow {
        1
    } else {
        CallNextHookEx(null_mut(), code, wparam, lparam)
    }
}

/// マウス操作は内容を記録せず、試験への介入として扱う。
unsafe extern "system" fn mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        EPOCH.fetch_add(1, Ordering::SeqCst);
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

/// 一度でも前面・フォーカスが動けば、元へ戻っても復元を再開しない。
unsafe extern "system" fn focus_event(
    _: HWINEVENTHOOK,
    _: u32,
    _: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    EPOCH.fetch_add(1, Ordering::SeqCst);
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

/// 対象 IME ウィンドウへの読み取り専用問い合わせを 50ms で打ち切る。
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

/// 通常版が同時起動されていないか、既知のウィンドウクラスだけで確認する。
fn normal_running() -> bool {
    [
        "Iris.Tray.Window",
        "Iris.Diagnostic.Window",
        "Iris.ImeTrial.Window",
    ]
    .iter()
    .any(|name| unsafe { !FindWindowW(wide(name).as_ptr(), null_mut()).is_null() })
}

/// 別スレッドで限定したメタデータだけを読み、状態の確定判断は行わない。
fn collect(snapshot: Snapshot) -> Report {
    let mut report = Report {
        snapshot,
        process: Err(0),
        focus_process: Err(0),
        ime_window: Err(0),
        layout: 0,
        open: ImeReply::NotQueried,
        conversion: ImeReply::NotQueried,
        status: "unavailable",
        elapsed_ms: 0,
    };
    if !snapshot.accepts(snapshot.id, unsafe { GetTickCount() }) {
        report.status = "deadline_exceeded";
        return report;
    }
    if normal_running() {
        report.status = "normal_iris_running; query_skipped";
        return report;
    }
    report.process = snapshot.foreground.and_then(|w| process_name(w.pid));
    report.focus_process = match (snapshot.foreground, snapshot.focus) {
        (Ok(fg), Ok(focus)) if fg.pid == focus.pid => report.process.clone(),
        (_, Ok(focus)) => process_name(focus.pid),
        (_, Err(code)) => Err(code),
    };
    if !supported(&report.process) {
        report.status = "unsupported_or_unidentified_process; query_skipped";
        return report;
    }
    let Ok(focus) = snapshot.focus else {
        report.status = "focus_unavailable; query_skipped";
        return report;
    };
    if !unchanged(snapshot) {
        report.status = "target_changed; query_skipped";
        return report;
    }
    report.layout = unsafe { GetKeyboardLayout(focus.tid) } as usize;
    report.ime_window = window_id(unsafe { ImmGetDefaultIMEWnd(focus.hwnd as HWND) });
    let Ok(ime) = report.ime_window else {
        report.status = "ime_window_unavailable; query_skipped";
        return report;
    };
    if ime.pid != focus.pid || ime.tid != focus.tid {
        report.status = "ime_owner_mismatch; query_skipped";
        return report;
    }
    if !snapshot.accepts(snapshot.id, unsafe { GetTickCount() }) {
        report.status = "deadline_exceeded";
        return report;
    }
    report.open = query(ime, IMC_GETOPENSTATUS);
    if unchanged(snapshot) && snapshot.accepts(snapshot.id, unsafe { GetTickCount() }) {
        report.conversion = query(ime, IMC_GETCONVERSIONMODE);
    }
    report.status = if unchanged(snapshot) {
        "sampled; before_after_identity_match"
    } else {
        "target_changed; replies_invalid"
    };
    report
}

/// 自分の注入イベントに、このプロセス内だけで使う識別値を付ける。
fn input_tag() -> usize {
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

struct TrialBackend {
    snapshot: Snapshot,
    epoch: u64,
    ime: WindowId,
    layout: usize,
}
impl Backend for TrialBackend {
    /// 各操作の直前に、対象・レイアウト・期限・介入を再確認する。
    fn guarded(&mut self) -> bool {
        let quick = || {
            !ABORT.load(Ordering::SeqCst)
                && EPOCH.load(Ordering::SeqCst) == self.epoch
                && model::within_deadline(self.snapshot.at, unsafe { GetTickCount() })
        };
        if !quick()
            || !unchanged(self.snapshot)
            || normal_running()
            || [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
                .iter()
                .any(|&vk| unsafe { GetAsyncKeyState(vk as i32) } < 0)
        {
            return false;
        }
        let (Ok(fg), Ok(focus)) = (self.snapshot.foreground, self.snapshot.focus) else {
            return false;
        };
        if !supported(&process_name(fg.pid)) || self.layout == 0 {
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
        CLOSING.load(Ordering::SeqCst)
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
                error: 0,
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
                error: GetLastError(),
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

/// 採取後に一件の入力比較を実行し、投入とアプリ側の成功を区別する。
fn trial(snapshot: Snapshot, epoch: u64) -> ResultReport {
    let mut report = collect(snapshot);
    report.elapsed_ms = unsafe { GetTickCount() }.wrapping_sub(snapshot.at);
    let mut text = report.render();
    let mut manual = false;
    if report.status == "sampled; before_after_identity_match"
        && matches!(report.open, ImeReply::Raw(0 | 1))
    {
        if let Ok(ime) = report.ime_window {
            let mut backend = TrialBackend {
                snapshot,
                epoch,
                ime,
                layout: report.layout,
            };
            let outcome = model::run(&mut backend);
            manual = outcome.manual;
            text.push_str(&outcome.render());
            if backend.guarded() {
                text.push_str(&format!(
                    "conversion_after_observation_only: {}\n",
                    query(ime, IMC_GETCONVERSIONMODE).describe()
                ));
            }
        }
    } else {
        text.push_str("trial_status=not_eligible; unchanged\n");
    }
    text.push_str(&format!(
        "trial_elapsed_ms={}\n",
        unsafe { GetTickCount() }.wrapping_sub(snapshot.at)
    ));
    ResultReport {
        snapshot,
        text,
        manual,
    }
}

/// 最新6件の結果を表示し、前面や入力フォーカスは動かさない。
fn append(text: String) {
    let view = APP.with(|cell| {
        let mut app = cell.borrow_mut();
        let app = app.as_mut()?;
        app.records.push(text);
        if app.records.len() > 6 {
            app.records.remove(0);
        }
        let text = format!("{}{}", INTRO, app.records.join("\n").replace('\n', "\r\n"));
        Some((app.edit, text))
    });
    if let Some((edit, text)) = view {
        unsafe {
            SetWindowTextW(edit, wide(&text).as_ptr());
        }
    }
}

/// F1 時点の識別情報付きで、採取できなかった理由だけを表示する。
fn failure(snapshot: Snapshot, reason: &str) {
    append(format!("--- sample #{} ---\nstatus={}\nforeground_at_f1: {}\nfocus_at_f1: {}\nIME=unknown; composition=unknown; no state inferred\n",
        snapshot.id, reason, identity(snapshot.foreground), identity(snapshot.focus)));
}

/// フックの予約を一件だけ作業スレッドへ渡す。
fn enqueue(id: usize) {
    let Some(snapshot) = HOOK.get().pending.filter(|s| s.id == id) else {
        return;
    };
    let sent = APP.with(|cell| {
        cell.borrow().as_ref().is_some_and(|a| {
            a.requests
                .try_send((snapshot, HOOK.get().pending_epoch))
                .is_ok()
        })
    });
    if !sent {
        let mut state = HOOK.get();
        state.pending = None;
        HOOK.set(state);
        failure(snapshot, "worker_busy_or_unavailable");
    }
}

/// 不明な状態では再試験を止め、手動確認を求める。
fn manual_notice() {
    LOCKED.store(true, Ordering::SeqCst);
    append("手動確認が必要です。入力・送信結果、キー状態、元の入力欄の IME を手動確認してください。再試験は手動確認後に終了・再起動してください。\n".into());
}

/// 終了時は復元の未確認を通知し、以後の処理を打ち切る。
fn finish_exit() {
    // 通知ダイアログのメッセージループから終了処理へ再入しない。
    if FINISHING.swap(true, Ordering::SeqCst) {
        return;
    }
    let mut state = HOOK.get();
    unsafe {
        KillTimer(state.window, POLL);
    }
    state.window = null_mut();
    state.pending = None;
    HOOK.set(state);
    ABORT.store(true, Ordering::SeqCst);
    if LOCKED.load(Ordering::SeqCst) {
        error("入力・復元結果は未確認です。送信結果・キー状態・IME を手動確認してください。");
    }
    unsafe {
        PostQuitMessage(0);
    }
}

/// 期限切れでも作業が終わるまで次の試験を受け付けず、終了待ちは有限にする。
fn poll() {
    let replies: Vec<_> = APP.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|a| a.replies.try_iter().collect())
            .unwrap_or_default()
    });
    for report in replies {
        let mut state = HOOK.get();
        if state.pending.is_some_and(|s| s.id == report.snapshot.id) {
            state.pending = None;
            HOOK.set(state);
            append(report.render());
            if report.manual {
                manual_notice();
            }
        }
    }
    let mut state = HOOK.get();
    if let Some(snapshot) = state.pending {
        if unsafe { GetTickCount() }.wrapping_sub(snapshot.at) > DEADLINE_MS
            && !state.timeout_reported
        {
            ABORT.store(true, Ordering::SeqCst);
            state.timeout_reported = true;
            HOOK.set(state);
            failure(snapshot, "trial_timeout; pending_worker_not_reused");
            manual_notice();
        }
    }
    if CLOSING.load(Ordering::SeqCst) {
        if state.pending.is_none() {
            finish_exit();
        } else if state
            .close_at
            .is_some_and(|at| unsafe { GetTickCount() }.wrapping_sub(at) >= 1000)
        {
            ABORT.store(true, Ordering::SeqCst);
            LOCKED.store(true, Ordering::SeqCst);
            finish_exit();
        }
    }
}

/// 診断ウィンドウ内の結果欄と選択ボタンの大きさだけを調整する。
fn layout(window: HWND) {
    let handles = APP.with(|cell| cell.borrow().as_ref().map(|a| (a.edit, a.button)));
    if let Some((edit, button)) = handles {
        unsafe {
            let mut rect = RECT::default();
            GetClientRect(window, &mut rect);
            MoveWindow(button, 10, 10, 180, 30, 1);
            MoveWindow(
                edit,
                10,
                50,
                (rect.right - 20).max(100),
                (rect.bottom - 60).max(100),
                1,
            );
        }
    }
}

/// 結果表示とユーザーによる選択操作だけを処理する。
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CAPTURE => enqueue(wparam),
        WM_TIMER if wparam == POLL => poll(),
        WM_SIZE => layout(window),
        WM_COMMAND if wparam & 0xFFFF == SELECT => {
            let edit = APP.with(|cell| cell.borrow().as_ref().map(|a| a.edit));
            if let Some(edit) = edit {
                // 自分の結果欄の選択だけ。外部アプリやクリップボードは操作しない。
                SetFocus(edit);
                SendMessageW(edit, EM_SETSEL, 0, -1);
            }
        }
        WM_CLOSE => {
            CLOSING.store(true, Ordering::SeqCst);
            let mut state = HOOK.get();
            state.close_at.get_or_insert_with(|| GetTickCount());
            HOOK.set(state);
            if state.pending.is_none() {
                finish_exit();
            } else {
                append(
                    "終了要求を受け付けました。対象と状態が保たれる場合のみ復元を試みます。\n"
                        .into(),
                );
            }
        }
        _ => return DefWindowProcW(window, message, wparam, lparam),
    }
    0
}

struct Resources {
    window: HWND,
    hook: HHOOK,
    mouse_hook: HHOOK,
    events: [HWINEVENTHOOK; 2],
}
impl Drop for Resources {
    /// 終了・初期化失敗のどちらでもフックと作業要求を解放する。
    fn drop(&mut self) {
        if HOOK.get().pending.is_some() {
            LOCKED.store(true, Ordering::SeqCst);
        }
        ABORT.store(true, Ordering::SeqCst);
        let mut state = HOOK.get();
        state.window = null_mut();
        state.pending = None;
        HOOK.set(state);
        APP.with(|cell| cell.borrow_mut().take());
        unsafe {
            for event in self.events {
                if !event.is_null() {
                    UnhookWinEvent(event);
                }
            }
            if !self.mouse_hook.is_null() {
                UnhookWindowsHookEx(self.mouse_hook);
            }
            if !self.hook.is_null() {
                UnhookWindowsHookEx(self.hook);
            }
            if !self.window.is_null() {
                KillTimer(self.window, POLL);
                DestroyWindow(self.window);
            }
        }
    }
}

/// 通常版と独立した UI・作業スレッド・F1 フックを開始する。
fn start() -> Result<(), &'static str> {
    unsafe {
        if normal_running() {
            return Err("通常版 Iris・読み取り専用診断版・切替試験版を終了してから入力比較版を起動してください。");
        }
        let class = wide("Iris.InputTrial.Window");
        if !FindWindowW(class.as_ptr(), null_mut()).is_null() {
            return Err("入力比較版は既に起動しています。");
        }
        let instance = GetModuleHandleW(null_mut());
        let definition = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..Default::default()
        };
        if RegisterClassW(&definition) == 0 {
            return Err("診断ウィンドウの登録に失敗しました。");
        }
        let mut resources = Resources {
            window: CreateWindowExW(
                0,
                class.as_ptr(),
                wide("Iris IME 入力比較 - OK と Enter を送信").as_ptr(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                920,
                680,
                null_mut(),
                null_mut(),
                instance,
                null_mut(),
            ),
            hook: null_mut(),
            mouse_hook: null_mut(),
            events: [null_mut(); 2],
        };
        if resources.window.is_null() {
            return Err("診断ウィンドウを作成できませんでした。");
        }
        let window = resources.window;
        let button = CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide("結果を選択 → Ctrl+C").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            10,
            10,
            180,
            30,
            window,
            SELECT as HMENU,
            instance,
            null_mut(),
        );
        let edit = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            wide("EDIT").as_ptr(),
            wide(INTRO).as_ptr(),
            WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WS_VSCROLL
                | ES_MULTILINE as u32
                | ES_READONLY as u32
                | ES_AUTOVSCROLL as u32,
            10,
            50,
            880,
            570,
            window,
            null_mut(),
            instance,
            null_mut(),
        );
        if button.is_null() || edit.is_null() {
            return Err("結果欄を作成できませんでした。");
        }
        let (tx, rx) = mpsc::sync_channel::<(Snapshot, u64)>(1);
        let (result_tx, result_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("iris-input-trial-worker".into())
            .spawn(move || {
                while let Ok((snapshot, epoch)) = rx.recv() {
                    let report = trial(snapshot, epoch);
                    if result_tx.send(report).is_err() {
                        break;
                    }
                }
            })
            .map_err(|_| "診断スレッドを開始できませんでした。")?;
        APP.with(|cell| {
            *cell.borrow_mut() = Some(App {
                edit,
                button,
                requests: tx,
                replies: result_rx,
                records: Vec::new(),
            })
        });
        let mut state = HOOK.get();
        state.window = window;
        state.gate.down = GetAsyncKeyState(VK_F1 as i32) < 0;
        HOOK.set(state);
        resources.hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), instance, 0);
        resources.mouse_hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), instance, 0);
        for (slot, event) in resources
            .events
            .iter_mut()
            .zip([EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_FOCUS])
        {
            *slot = SetWinEventHook(
                event,
                event,
                null_mut(),
                Some(focus_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
        }
        if resources.hook.is_null()
            || resources.mouse_hook.is_null()
            || resources.events.iter().any(|h| h.is_null())
            || SetTimer(window, POLL, 50, None) == 0
        {
            return Err("F1 または結果確認タイマーを開始できませんでした。");
        }
        layout(window);
        ShowWindow(window, SW_SHOW);
        let mut message = MSG::default();
        loop {
            match GetMessageW(&mut message, null_mut(), 0, 0) {
                -1 => return Err("メッセージを取得できませんでした。"),
                0 => break,
                _ => {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
        Ok(())
    }
}

/// 診断起動の失敗だけを表示する。
pub fn run() {
    if let Err(message) = start() {
        error(message);
        if LOCKED.load(Ordering::SeqCst) {
            error("試験が途中で終了しました。元の入力欄で IME を手動確認・復元してください。");
        }
    }
}
