use crate::{
    diagnostic::WindowId,
    ime::{self, Completion, Request},
    logic::{Decision, F1State, Pending},
    model,
};
use std::{
    cell::{Cell, RefCell},
    mem::size_of,
    ptr::null_mut,
    sync::{
        atomic::Ordering,
        mpsc::{self, Receiver, SyncSender},
    },
};
use windows_sys::Win32::{
    Foundation::*,
    System::{LibraryLoader::GetModuleHandleW, SystemInformation::GetTickCount},
    UI::{Accessibility::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

const WM_TRAY: u32 = WM_APP + 1;
const WM_SEND: u32 = WM_APP + 2;
const WM_REFRESH: u32 = WM_APP + 3;
const TOGGLE: u32 = 1;
const EXIT: u32 = 2;
const REFRESH: usize = 1;
const POLL: usize = 2;

#[derive(Clone, Copy)]
struct State {
    window: HWND,
    enabled: bool,
    target: Option<WindowId>,
    pending: Pending<Request>,
    timed_out: bool,
    f1: F1State,
    taskbar_created: u32,
    closing_at: Option<u32>,
    finishing: bool,
    notifying: bool,
}
struct Worker {
    requests: SyncSender<Request>,
    replies: Receiver<Completion>,
}
thread_local! {
    static STATE: Cell<State> = const { Cell::new(State {
        window: null_mut(), enabled: true, target: None, pending: Pending::EMPTY,
        timed_out: false, f1: F1State::EMPTY, taskbar_created: 0,
        closing_at: None, finishing: false, notifying: false,
    }) };
    static WORKER: RefCell<Option<Worker>> = const { RefCell::new(None) };
}

/// Win32 用の終端付き文字列を作る。
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// コールバック中に借用を保持せず状態を更新する。
fn update<R>(change: impl FnOnce(&mut State) -> R) -> R {
    let mut state = STATE.get();
    let result = change(&mut state);
    STATE.set(state);
    result
}

/// 初期化または入力・復元を確認できない理由を表示する。
fn error(text: &str) {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(text).as_ptr(),
            wide("Iris").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// 無効化と表示を一緒に行い、通知中に再開させない。
fn report_error(text: &str) {
    ime::STOP.store(true, Ordering::SeqCst);
    update(|s| {
        s.enabled = false;
        s.notifying = true;
    });
    tray(NIM_MODIFY);
    error(text);
    update(|s| s.notifying = false);
}

/// プロセス名の照会はフック外で行い、候補が変われば現在の処理を失効させる。
fn refresh() {
    let current = ime::foreground();
    let target = current
        .filter(|&w| ime::is_allowed(w))
        .filter(|_| ime::foreground() == current);
    if STATE.get().target != target {
        ime::EPOCH.fetch_add(1, Ordering::SeqCst);
    }
    update(|s| s.target = target);
}

/// 修飾キーの押下を確認する。
fn modifiers_down() -> bool {
    [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|&vk| unsafe { GetAsyncKeyState(vk as i32) } < 0)
}

/// 前面・フォーカス変更を失効として記録し、名前照会は後で行う。
unsafe extern "system" fn focus_event(
    _: HWINEVENTHOOK,
    event: u32,
    _: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    ime::EPOCH.fetch_add(1, Ordering::SeqCst);
    if event == EVENT_SYSTEM_FOREGROUND {
        update(|s| s.target = None);
        PostMessageW(STATE.get().window, WM_REFRESH, 0, 0);
    }
}

/// マウス操作の内容は保存せず、介入だけを記録する。
unsafe extern "system" fn mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        ime::EPOCH.fetch_add(1, Ordering::SeqCst);
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

/// 物理 F1 の対象判定だけを行い、IME 処理・待機をフック内で行わない。
unsafe extern "system" fn keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    let key = &*(lparam as *const KBDLLHOOKSTRUCT);
    let injected = key.flags & LLKHF_INJECTED != 0;
    if model::own_input(injected, key.dwExtraInfo, ime::input_tag()) {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    if injected || key.vkCode != VK_F1 as u32 {
        ime::EPOCH.fetch_add(1, Ordering::SeqCst);
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    let down = match wparam as u32 {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_KEYUP | WM_SYSKEYUP => false,
        _ => return CallNextHookEx(null_mut(), code, wparam, lparam),
    };
    let mut state = STATE.get();
    let target = state.target.filter(|&w| ime::foreground() == Some(w));
    let eligible = state.enabled
        && state.closing_at.is_none()
        && !state.finishing
        && target.is_some()
        && !modifiers_down()
        && key.flags & LLKHF_ALTDOWN == 0;
    let decision = state.f1.event(down, eligible, false);
    if decision == Decision::Send {
        if let Some(target) = target {
            // 対象 F1 の連打は抑止だけ行い、処理完了まで予約を重ねない。
            if state.pending.current().is_none() {
                let request = ime::capture(target, key.time);
                if let Some(id) = state.pending.reserve(request) {
                    state.timed_out = false;
                    ime::ABORT.store(false, Ordering::SeqCst);
                    ime::STOP.store(false, Ordering::SeqCst);
                    if PostMessageW(state.window, WM_SEND, id, 0) == 0 {
                        state.pending.finish(id);
                        state.f1 = F1State::EMPTY;
                        state.f1.event(true, false, false);
                        STATE.set(state);
                        return CallNextHookEx(null_mut(), code, wparam, lparam);
                    }
                }
            }
        }
    }
    STATE.set(state);
    if decision == Decision::Pass {
        CallNextHookEx(null_mut(), code, wparam, lparam)
    } else {
        1
    }
}

/// 作業を一度だけ渡し、重複した古いメッセージは無視する。
fn enqueue(id: usize) {
    let Some(mut request) = update(|s| s.pending.start(id)) else {
        return;
    };
    request.snapshot.id = id;
    let sent = WORKER.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|w| w.requests.try_send(request).is_ok())
    });
    if !sent {
        update(|s| {
            s.pending.finish(id);
        });
        report_error("入力処理を開始できないため Iris を無効にしました。送信していません。");
    }
}

/// 終了は新しい入力を止め、処理中なら条件付き復元の完了を有限時間待つ。
fn request_exit() {
    ime::STOP.store(true, Ordering::SeqCst);
    update(|s| {
        s.enabled = false;
        s.closing_at
            .get_or_insert_with(|| unsafe { GetTickCount() });
    });
    tray(NIM_MODIFY);
    if STATE.get().pending.current().is_none() {
        finish_exit(false);
    }
}

/// 終了通知への再入を防ぎ、未完了なら手動確認を知らせる。
fn finish_exit(uncertain: bool) {
    if update(|s| {
        let old = s.finishing;
        s.finishing = true;
        old
    }) {
        return;
    }
    ime::ABORT.store(true, Ordering::SeqCst);
    unsafe {
        KillTimer(STATE.get().window, POLL);
        KillTimer(STATE.get().window, REFRESH);
    }
    if uncertain {
        error("処理完了・IME 復元を確認できません。入力・送信結果、キー状態、元の入力欄の IME を手動確認・復元してください。");
    }
    unsafe {
        PostQuitMessage(0);
    }
}

/// 完了通知だけで予約を解放し、期限切れでも未完了の処理を再利用しない。
fn poll() {
    let replies: Vec<_> = WORKER.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|w| w.replies.try_iter().collect())
            .unwrap_or_default()
    });
    for result in replies {
        if update(|s| s.pending.finish(result.id)) {
            let warning = result.warning.or(if STATE.get().timed_out {
                Some("処理が期限を超えたため Iris を無効にしました。入力・送信結果、キー状態、IME を手動確認してください。")
            } else { None });
            if let Some(warning) = warning {
                report_error(warning);
            }
        }
    }
    let state = STATE.get();
    if let Some((_, request)) = state.pending.current() {
        if !state.timed_out && ime::expired(request) {
            ime::ABORT.store(true, Ordering::SeqCst);
            ime::STOP.store(true, Ordering::SeqCst);
            update(|s| {
                s.timed_out = true;
                s.enabled = false;
            });
            tray(NIM_MODIFY);
            notify_timeout();
            // 別スレッドが戻る前にはモーダル表示でフォーカスを動かさない。
        }
    }
    let state = STATE.get();
    if let Some(at) = state.closing_at {
        if state.pending.current().is_none() {
            finish_exit(false);
        } else if unsafe { GetTickCount() }.wrapping_sub(at) >= 1000 {
            finish_exit(true);
        }
    }
}

/// 通知領域アイコンの状態を登録・更新・削除する。
fn tray(action: u32) -> bool {
    let state = STATE.get();
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: state.window,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: unsafe { LoadIconW(null_mut(), IDI_APPLICATION) },
        ..Default::default()
    };
    let tip = wide(if state.timed_out && state.pending.current().is_some() {
        "Iris: 無効・処理未確認（手動確認が必要）"
    } else if state.enabled {
        "Iris: 有効"
    } else {
        "Iris: 無効"
    });
    data.szTip[..tip.len()].copy_from_slice(&tip);
    unsafe { Shell_NotifyIconW(action, &data) != 0 }
}

/// 処理が戻らない場合も、フォーカスを奪わず手動確認の必要を知らせる。
fn notify_timeout() {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: STATE.get().window,
        uID: 1,
        uFlags: NIF_INFO,
        dwInfoFlags: NIIF_WARNING,
        ..Default::default()
    };
    let title = wide("Iris: 処理・復元が未確認");
    let message = wide("期限を超えたため無効にしました。入力・送信結果、キー状態、IME を手動確認してください。自動再送はしません。");
    data.szInfoTitle[..title.len()].copy_from_slice(&title);
    data.szInfo[..message.len()].copy_from_slice(&message);
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

/// 通知領域メニューから有効状態の切替と終了を行う。
fn menu(window: HWND) {
    if STATE.get().notifying || STATE.get().finishing || STATE.get().closing_at.is_some() {
        return;
    }
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        let desired_enabled = !STATE.get().enabled;
        let checked = if !desired_enabled {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        AppendMenuW(
            menu,
            MF_STRING
                | checked
                | if !STATE.get().enabled && STATE.get().pending.current().is_some() {
                    MF_GRAYED
                } else {
                    0
                },
            TOGGLE as usize,
            wide("有効").as_ptr(),
        );
        AppendMenuW(menu, MF_STRING, EXIT as usize, wide("終了").as_ptr());

        let mut point = POINT::default();
        GetCursorPos(&mut point);
        SetForegroundWindow(window);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            0,
            window,
            null_mut(),
        );
        DestroyMenu(menu);
        PostMessageW(window, WM_NULL, 0, 0);

        match command as u32 {
            TOGGLE => {
                // メニュー中に失敗通知が来ても、「無効にする」を再有効化へ反転させない。
                if STATE.get().closing_at.is_none()
                    && (!desired_enabled
                        || (STATE.get().pending.current().is_none() && !STATE.get().notifying))
                {
                    ime::STOP.store(true, Ordering::SeqCst);
                    update(|state| state.enabled = desired_enabled);
                    tray(NIM_MODIFY);
                }
            }
            EXIT => {
                request_exit();
            }
            _ => {}
        }
    }
}

/// 隠しウィンドウで通知領域と処理結果を受ける。
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let taskbar_created = STATE.get().taskbar_created;
    if taskbar_created != 0 && message == taskbar_created {
        if !tray(NIM_ADD) {
            request_exit();
        }
        return 0;
    }
    match message {
        WM_SEND => enqueue(wparam),
        WM_REFRESH => refresh(),
        WM_TIMER if wparam == REFRESH => refresh(),
        WM_TIMER if wparam == POLL => poll(),
        WM_TRAY if lparam as u32 == WM_RBUTTONUP || lparam as u32 == WM_LBUTTONUP => menu(window),
        WM_CLOSE => request_exit(),
        _ => return DefWindowProcW(window, message, wparam, lparam),
    }
    0
}

struct Resources {
    window: HWND,
    keyboard: HHOOK,
    mouse: HHOOK,
    events: [HWINEVENTHOOK; 2],
}
impl Drop for Resources {
    /// 初期化失敗・終了ともフックと通知領域を片付ける。強制終了の復元は保証しない。
    fn drop(&mut self) {
        ime::ABORT.store(true, Ordering::SeqCst);
        ime::STOP.store(true, Ordering::SeqCst);
        update(|s| s.enabled = false);
        WORKER.with(|cell| cell.borrow_mut().take());
        unsafe {
            if !self.keyboard.is_null() {
                UnhookWindowsHookEx(self.keyboard);
            }
            if !self.mouse.is_null() {
                UnhookWindowsHookEx(self.mouse);
            }
            for event in self.events {
                if !event.is_null() {
                    UnhookWinEvent(event);
                }
            }
            if !self.window.is_null() {
                KillTimer(self.window, REFRESH);
                KillTimer(self.window, POLL);
                tray(NIM_DELETE);
                DestroyWindow(self.window);
            }
        }
    }
}

/// 隠しウィンドウ・作業スレッド・フック・通知領域を作る。
fn start() -> Result<(), &'static str> {
    unsafe {
        let class = wide("Iris.Tray.Window");
        if !FindWindowW(class.as_ptr(), null_mut()).is_null() || ime::other_iris_running() {
            return Err("他の Iris（通常版・診断版・試験版）を終了してから起動してください。");
        }
        let instance = GetModuleHandleW(null_mut());
        let definition = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        };
        if RegisterClassW(&definition) == 0 {
            return Err("ウィンドウの登録に失敗しました。");
        }
        let mut resources = Resources {
            window: CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                null_mut(),
            ),
            keyboard: null_mut(),
            mouse: null_mut(),
            events: [null_mut(); 2],
        };
        if resources.window.is_null() {
            return Err("ウィンドウの作成に失敗しました。");
        }
        update(|s| {
            s.window = resources.window;
            s.f1.down = GetAsyncKeyState(VK_F1 as i32) < 0;
            s.taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        });
        let (tx, rx) = mpsc::sync_channel::<Request>(1);
        let (result_tx, result_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("iris-input-worker".into())
            .spawn(move || {
                while let Ok(request) = rx.recv() {
                    if result_tx.send(ime::execute(request)).is_err() {
                        break;
                    }
                }
            })
            .map_err(|_| "作業スレッドを開始できませんでした。")?;
        WORKER.with(|cell| {
            *cell.borrow_mut() = Some(Worker {
                requests: tx,
                replies: result_rx,
            })
        });
        refresh();
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
        resources.keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), instance, 0);
        resources.mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), instance, 0);
        if resources.events.iter().any(|h| h.is_null())
            || resources.keyboard.is_null()
            || resources.mouse.is_null()
        {
            return Err("入力フックの登録に失敗しました。");
        }
        if SetTimer(resources.window, REFRESH, 200, None) == 0
            || SetTimer(resources.window, POLL, 50, None) == 0
            || !tray(NIM_ADD)
        {
            return Err("通知領域またはタイマーの初期化に失敗しました。");
        }
        let mut message = MSG::default();
        loop {
            match GetMessageW(&mut message, null_mut(), 0, 0) {
                -1 => return Err("メッセージ取得に失敗しました。処理中だった場合は入力・送信結果、キー状態、IME を手動確認してください。"),
                0 => break,
                _ => { TranslateMessage(&message); DispatchMessageW(&message); }
            }
        }
        Ok(())
    }
}

/// 起動または継続に失敗した場合は、理由を表示して終了する。
pub fn run() {
    if let Err(message) = start() {
        error(message);
    }
}
