use std::{cell::Cell, mem::size_of, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::*,
    System::{LibraryLoader::GetModuleHandleW, SystemInformation::GetTickCount, Threading::*},
    UI::{Accessibility::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

use crate::logic::{allowed_image, Decision, F1State, PendingSend, ENTER_DELAY_MS};

const WM_TRAY: u32 = WM_APP + 1;
const WM_SEND: u32 = WM_APP + 2;
const WM_REFRESH: u32 = WM_APP + 3;
const TOGGLE: u32 = 1;
const EXIT: u32 = 2;
const TIMER: usize = 1;
const INPUT_TAG: usize = 0x49524953;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Target {
    window: HWND,
    process: u32,
    thread: u32,
}

#[derive(Clone, Copy)]
struct State {
    window: HWND,
    enabled: bool,
    target: Option<Target>,
    pending: PendingSend<Target>,
    f1: F1State,
    taskbar_created: u32,
}

thread_local! {
    // コールバックはすべて登録した UI スレッドで実行される。
    static STATE: Cell<State> = const { Cell::new(State {
        window: null_mut(),
        enabled: true,
        target: None,
        pending: PendingSend::EMPTY,
        f1: F1State::EMPTY,
        taskbar_created: 0,
    }) };
}

/// Win32 用の終端付き UTF-16 文字列を作る。
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// コールバック中に借用を保持せず状態を更新する。
fn update<R>(change: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|cell| {
        let mut state = cell.get();
        let result = change(&mut state);
        cell.set(state);
        result
    })
}

/// 起動時や送信失敗時のエラーだけを表示する。
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

/// 現在の前面ウィンドウと所属プロセスを取得する。
fn foreground() -> Option<Target> {
    unsafe {
        let window = GetForegroundWindow();
        let mut process = 0;
        let thread = GetWindowThreadProcessId(window, &mut process);
        if window.is_null() || process == 0 || thread == 0 {
            None
        } else {
            Some(Target {
                window,
                process,
                thread,
            })
        }
    }
}

/// プロセス照会はフックの外で行い、失敗した場合は対象にしない。
fn is_allowed(target: Target) -> bool {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, target.process);
        if process.is_null() {
            return false;
        }

        let mut path = [0u16; 32768];
        let mut length = path.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length);
        CloseHandle(process);

        ok != 0 && String::from_utf16(&path[..length as usize]).is_ok_and(|s| allowed_image(&s))
    }
}

/// 前面が変わった場合は待機中の送信を捨て、対象を更新する。
fn refresh() {
    let current = foreground();
    let target = current.filter(|&target| is_allowed(target));
    let target = target.filter(|_| foreground() == current);
    if STATE.get().target != target {
        cancel_pending();
    }
    update(|state| state.target = target);
}

/// Shift、Ctrl、Alt、Windows キーの押下を確認する。
fn modifiers_down() -> bool {
    [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|&key| unsafe { GetAsyncKeyState(key as i32) < 0 })
}

/// 前面変更通知では候補を無効化し、名前照会をメッセージへ回す。
unsafe extern "system" fn foreground_event(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    cancel_pending();
    update(|state| state.target = None);
    PostMessageW(STATE.get().window, WM_REFRESH, 0, 0);
}

/// フックは物理 F1 の判定と送信予約だけを行う。
unsafe extern "system" fn keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }

    let key = &*(lparam as *const KBDLLHOOKSTRUCT);
    if key.flags & LLKHF_INJECTED != 0 {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }

    let down = match wparam as u32 {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_KEYUP | WM_SYSKEYUP => false,
        _ => return CallNextHookEx(null_mut(), code, wparam, lparam),
    };
    // 待機中に押した修飾キーは、Enter までに離されても予約を取り消す。
    if down
        && [
            VK_SHIFT,
            VK_LSHIFT,
            VK_RSHIFT,
            VK_CONTROL,
            VK_LCONTROL,
            VK_RCONTROL,
            VK_MENU,
            VK_LMENU,
            VK_RMENU,
            VK_LWIN,
            VK_RWIN,
        ]
        .contains(&(key.vkCode as u16))
    {
        cancel_pending();
    }
    if key.vkCode != VK_F1 as u32 {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }

    let mut state = STATE.get();
    let target = state.target.filter(|&target| foreground() == Some(target));
    let eligible =
        state.enabled && target.is_some() && !modifiers_down() && key.flags & LLKHF_ALTDOWN == 0;
    let decision = state.f1.event(down, eligible, false);

    if decision == Decision::Send {
        // 待機中の対象 F1 は抑止だけ行い、予約を重ねない。
        if let Some(id) = target.and_then(|target| state.pending.reserve(target, key.time)) {
            if PostMessageW(state.window, WM_SEND, id, 0) == 0 {
                state.pending.cancel();
                state.f1 = F1State::default();
                state.f1.event(true, false, false);
                STATE.set(state);
                return CallNextHookEx(null_mut(), code, wparam, lparam);
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

/// Unicode 文字または仮想キーの入力イベントを作る。
fn input(key: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: INPUT_TAG,
            },
        },
    }
}

/// 保留とタイマーを取り消す。既に届いた通知は予約の識別子で拒否する。
fn cancel_pending() {
    let id = update(|state| state.pending.cancel());
    if let Some(id) = id {
        unsafe {
            KillTimer(STATE.get().window, id);
        }
    }
}

/// 完了した予約だけを解放し、再入で作られた別の予約を残す。
fn finish_pending(id: usize) {
    update(|state| state.pending.finish(id));
    unsafe {
        KillTimer(STATE.get().window, id);
    }
}

/// 送信直前の有効状態、修飾キーと元の前面アプリを確認する。
fn ready(target: Target) -> bool {
    is_allowed(target) && STATE.get().enabled && !modifiers_down() && foreground() == Some(target)
}

/// 入力の投入数を確認し、失敗時は保留を破棄して無効化する。
fn send_inputs(events: &[INPUT]) -> bool {
    let sent = unsafe {
        SendInput(
            events.len() as u32,
            events.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent == events.len() as u32 {
        return true;
    }

    cancel_pending();
    update(|state| state.enabled = false);
    tray(NIM_MODIFY);
    error("入力の送信に失敗したため Iris を無効にしました。\n一部だけ入力された可能性があります。入力欄とキー状態を確認してください。自動再送はしません。");
    false
}

/// 文字だけを送り、成功後にフック外で 100ms のタイマーを開始する。
fn send_text(id: usize) {
    let Some(target) = STATE.get().pending.target(id) else {
        return;
    };
    let events = [
        input(0, 'O' as u16, KEYEVENTF_UNICODE),
        input(0, 'O' as u16, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
        input(0, 'K' as u16, KEYEVENTF_UNICODE),
        input(0, 'K' as u16, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
    ];
    let allowed = ready(target);
    let now = unsafe { GetTickCount() };
    if !update(|state| state.pending.begin_text(id, now, allowed)) {
        return;
    }
    if !send_inputs(&events) {
        return;
    }

    // SendInput 中の取消・再入で予約が変わっていれば Enter を予約しない。
    let now = unsafe { GetTickCount() };
    if !update(|state| state.pending.text_sent(id, now)) {
        return;
    }
    let window = STATE.get().window;
    if unsafe { SetTimer(window, id, ENTER_DELAY_MS, None) } == 0 {
        cancel_pending();
        update(|state| state.enabled = false);
        tray(NIM_MODIFY);
        error("Enter の待機に失敗したため Iris を無効にしました。入力欄の OK を確認してください。");
    } else if STATE.get().pending.target(id).is_none() {
        unsafe {
            KillTimer(window, id);
        }
    }
}

/// タイマーの重複通知を拒否し、条件が保たれた場合だけ Enter を送る。
fn send_enter(id: usize) {
    let Some(target) = STATE.get().pending.target(id) else {
        unsafe {
            KillTimer(STATE.get().window, id);
        }
        return;
    };
    let events = [input(VK_RETURN, 0, 0), input(VK_RETURN, 0, KEYEVENTF_KEYUP)];
    let allowed = ready(target);
    let now = unsafe { GetTickCount() };
    if !update(|state| state.pending.begin_enter(id, now, allowed)) {
        if STATE.get().pending.target(id).is_none() {
            unsafe {
                KillTimer(STATE.get().window, id);
            }
        }
        return;
    }

    // SendInput 中も予約を保持して二重送信を防ぐ。送信先切替の競合は残る。
    unsafe {
        KillTimer(STATE.get().window, id);
    }
    send_inputs(&events);
    finish_pending(id);
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
    let tip = wide(if state.enabled {
        "Iris: 有効"
    } else {
        "Iris: 無効"
    });
    data.szTip[..tip.len()].copy_from_slice(&tip);
    unsafe { Shell_NotifyIconW(action, &data) != 0 }
}

/// 通知領域メニューから有効状態の切替と終了を行う。
fn menu(window: HWND) {
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        let checked = if STATE.get().enabled {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        AppendMenuW(
            menu,
            MF_STRING | checked,
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
                cancel_pending();
                update(|state| state.enabled = !state.enabled);
                tray(NIM_MODIFY);
            }
            EXIT => {
                cancel_pending();
                update(|state| state.enabled = false);
                PostQuitMessage(0);
            }
            _ => {}
        }
    }
}

/// 隠しウィンドウでフック外の処理と通知領域操作を受ける。
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let taskbar_created = STATE.get().taskbar_created;
    if taskbar_created != 0 && message == taskbar_created {
        if !tray(NIM_ADD) {
            cancel_pending();
            update(|state| state.enabled = false);
            PostQuitMessage(1);
        }
        return 0;
    }

    match message {
        WM_SEND => send_text(wparam),
        WM_REFRESH => refresh(),
        WM_TIMER if wparam == TIMER => refresh(),
        WM_TIMER => send_enter(wparam),
        WM_TRAY if lparam as u32 == WM_RBUTTONUP || lparam as u32 == WM_LBUTTONUP => menu(window),
        WM_CLOSE => {
            cancel_pending();
            update(|state| state.enabled = false);
            PostQuitMessage(0);
        }
        _ => return DefWindowProcW(window, message, wparam, lparam),
    }
    0
}

/// 所有するフックと通知領域アイコンを終了時に解放する。
struct Resources {
    window: HWND,
    keyboard: HHOOK,
    foreground: HWINEVENTHOOK,
}

impl Drop for Resources {
    /// 起動途中の失敗も通常終了と同じ手順で片付ける。
    fn drop(&mut self) {
        cancel_pending();
        update(|state| state.enabled = false);
        unsafe {
            if !self.keyboard.is_null() {
                UnhookWindowsHookEx(self.keyboard);
            }
            if !self.foreground.is_null() {
                UnhookWinEvent(self.foreground);
            }
            if !self.window.is_null() {
                KillTimer(self.window, TIMER);
                tray(NIM_DELETE);
                DestroyWindow(self.window);
            }
        }
    }
}

/// ウィンドウとフックを作成し、メッセージループを開始する。
fn start() -> Result<(), &'static str> {
    unsafe {
        let instance = GetModuleHandleW(null_mut());
        let class = wide("Iris.Tray.Window");
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
            foreground: null_mut(),
        };
        if resources.window.is_null() {
            return Err("ウィンドウの作成に失敗しました。");
        }
        update(|state| {
            state.window = resources.window;
            state.f1.down = GetAsyncKeyState(VK_F1 as i32) < 0;
            state.taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        });
        refresh();

        resources.foreground = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            null_mut(),
            Some(foreground_event),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        resources.keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), instance, 0);
        if resources.foreground.is_null() || resources.keyboard.is_null() {
            return Err("入力フックの登録に失敗しました。");
        }
        if SetTimer(resources.window, TIMER, 200, None) == 0 || !tray(NIM_ADD) {
            return Err("通知領域またはタイマーの初期化に失敗しました。");
        }

        let mut message = MSG::default();
        loop {
            match GetMessageW(&mut message, null_mut(), 0, 0) {
                -1 => return Err("メッセージの取得に失敗しました。"),
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

/// 初期化に失敗した場合は常駐せず、理由を表示する。
pub fn run() {
    if let Err(message) = start() {
        error(message);
    }
}
