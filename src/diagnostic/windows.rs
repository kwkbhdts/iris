use std::{
    cell::{Cell, RefCell},
    mem::size_of,
    ptr::null_mut,
    sync::mpsc::{self, Receiver, SyncSender},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{
        LibraryLoader::GetModuleHandleW, ProcessStatus::K32GetModuleBaseNameW,
        SystemInformation::GetTickCount, Threading::*,
    },
    UI::{
        Controls::EM_SETSEL,
        Input::{Ime::ImmGetDefaultIMEWnd, KeyboardAndMouse::*},
        WindowsAndMessaging::*,
    },
};

use super::model::*;

const WM_CAPTURE: u32 = WM_APP + 1;
const SELECT: usize = 1;
const POLL: usize = 1;
const QUERY_TIMEOUT_MS: u32 = 50;
// 従来の IMM 互換問い合わせ。対応は保証せず、生の応答だけを記録する。
const IMC_GETCONVERSIONMODE: usize = 0x0001;
const IMC_GETOPENSTATUS: usize = 0x0005;
const INTRO: &str = "Iris 診断版（読み取りのみ）\r\n通常版 iris.exe は終了してください。\r\n対象の入力欄で、半角 → 日本語 → 変換途中の順に単独 F1 を各1回。\r\n結果が出るまで次の F1 は押さないでください。最新6件を表示します。\r\n「結果を選択」→ Ctrl+C でコピーできます。終了は右上の ×。\r\nraw=0 は IME オフの証明ではありません。変換中の有無は常に不明です。\r\n\r\n";

#[derive(Clone, Copy)]
struct HookState {
    window: HWND,
    gate: F1Gate,
    pending: Option<Snapshot>,
    next_id: usize,
}

struct App {
    edit: HWND,
    button: HWND,
    requests: SyncSender<Snapshot>,
    replies: Receiver<Report>,
    records: Vec<String>,
}

thread_local! {
    static HOOK: Cell<HookState> = const { Cell::new(HookState {
        window: null_mut(), gate: F1Gate::EMPTY, pending: None, next_id: 1,
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
            wide("Iris 診断").as_ptr(),
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
    if key.vkCode != VK_F1 as u32 || key.flags & LLKHF_INJECTED != 0 {
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
    let (swallow, capture) = state.gate.event(down, modified, state.pending.is_some());
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
fn query(window: WindowId, command: usize) -> ImeReply {
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
            0,
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

/// 通常版が同時起動されていないか、既知のウィンドウクラスだけで確認する。
fn normal_running() -> bool {
    unsafe { !FindWindowW(wide("Iris.Tray.Window").as_ptr(), null_mut()).is_null() }
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
        cell.borrow()
            .as_ref()
            .is_some_and(|a| a.requests.try_send(snapshot).is_ok())
    });
    if !sent {
        let mut state = HOOK.get();
        state.pending = None;
        HOOK.set(state);
        failure(snapshot, "worker_busy_or_unavailable");
    }
}

/// 応答は短い UI タイマーで回収し、500ms を超えた結果は採用しない。
fn poll() {
    let replies: Vec<_> = APP.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|a| a.replies.try_iter().collect())
            .unwrap_or_default()
    });
    for report in replies {
        let mut state = HOOK.get();
        if state
            .pending
            .is_some_and(|s| s.accepts(report.snapshot.id, unsafe { GetTickCount() }))
        {
            state.pending = None;
            HOOK.set(state);
            append(report.render());
        }
    }
    let mut state = HOOK.get();
    if let Some(snapshot) = state.pending {
        if !snapshot.accepts(snapshot.id, unsafe { GetTickCount() }) {
            state.pending = None;
            HOOK.set(state);
            failure(snapshot, "capture_timeout_500ms; late_result_discarded");
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
            let mut state = HOOK.get();
            state.window = null_mut();
            state.pending = None;
            HOOK.set(state);
            PostQuitMessage(0);
        }
        _ => return DefWindowProcW(window, message, wparam, lparam),
    }
    0
}

struct Resources {
    window: HWND,
    hook: HHOOK,
}
impl Drop for Resources {
    /// 終了・初期化失敗のどちらでもフックと作業要求を解放する。
    fn drop(&mut self) {
        let mut state = HOOK.get();
        state.window = null_mut();
        state.pending = None;
        HOOK.set(state);
        APP.with(|cell| cell.borrow_mut().take());
        unsafe {
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
            return Err("通常版 Iris を通知領域の「終了」で閉じてから診断版を起動してください。");
        }
        let class = wide("Iris.Diagnostic.Window");
        if !FindWindowW(class.as_ptr(), null_mut()).is_null() {
            return Err("診断版は既に起動しています。");
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
                wide("Iris 診断 - 読み取りのみ").as_ptr(),
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
        let (tx, rx) = mpsc::sync_channel::<Snapshot>(1);
        let (result_tx, result_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("iris-diagnostic-reader".into())
            .spawn(move || {
                while let Ok(snapshot) = rx.recv() {
                    let mut report = collect(snapshot);
                    report.elapsed_ms = GetTickCount().wrapping_sub(snapshot.at);
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
        if resources.hook.is_null() || SetTimer(window, POLL, 50, None) == 0 {
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
    }
}
