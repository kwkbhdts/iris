use std::{cell::Cell, mem::size_of, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::*,
    System::{LibraryLoader::GetModuleHandleW, SystemInformation::GetTickCount, Threading::*},
    UI::{Accessibility::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

use crate::logic::{allowed_image, Decision, F1State};

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
struct Request {
    target: Target,
    time: u32,
}

#[derive(Clone, Copy)]
struct State {
    window: HWND,
    enabled: bool,
    target: Option<Target>,
    pending: Option<Request>,
    f1: F1State,
    taskbar_created: u32,
}

thread_local! {
    // コールバックはすべて登録した UI スレッドで実行される。
    static STATE: Cell<State> = const { Cell::new(State {
        window: null_mut(),
        enabled: true,
        target: None,
        pending: None,
        f1: F1State::EMPTY,
        taskbar_created: 0,
    }) };
}

/// Win32 用の終端付き UTF-16 文字列を作る。
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// コールバック中に借用を保持せず状態を更新する。
fn update(change: impl FnOnce(&mut State)) {
    STATE.with(|cell| {
        let mut state = cell.get();
        change(&mut state);
        cell.set(state);
    });
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
    update(|state| {
        if state.target != target {
            state.pending = None;
        }
        state.target = target;
    });
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
    update(|state| {
        state.target = None;
        state.pending = None;
    });
    PostMessageW(STATE.get().window, WM_REFRESH, 0, 0);
}

/// フックは物理 F1 の判定と送信予約だけを行う。
unsafe extern "system" fn keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
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
    let mut state = STATE.get();
    let target = state.target.filter(|&target| foreground() == Some(target));
    let eligible = state.enabled
        && state.pending.is_none()
        && target.is_some()
        && !modifiers_down()
        && key.flags & LLKHF_ALTDOWN == 0;
    let decision = state.f1.event(down, eligible, false);

    if decision == Decision::Send {
        // 予約に失敗したときは、この押下を通常の F1 として通す。
        if PostMessageW(state.window, WM_SEND, 0, 0) == 0 {
            state.f1 = F1State::default();
            state.f1.event(true, false, false);
            STATE.set(state);
            return CallNextHookEx(null_mut(), code, wparam, lparam);
        }
        state.pending = target.map(|target| Request {
            target,
            time: key.time,
        });
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

/// 遅れた予約を捨て、前面を再確認して一括で OK と Enter を送る。
fn send_pending() {
    let state = STATE.get();
    update(|state| state.pending = None);
    let Some(request) = state.pending else {
        return;
    };
    if !state.enabled || unsafe { GetTickCount() }.wrapping_sub(request.time) > 250 {
        return;
    }
    if !is_allowed(request.target) {
        return;
    }

    let events = [
        input(0, 'O' as u16, KEYEVENTF_UNICODE),
        input(0, 'O' as u16, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
        input(0, 'K' as u16, KEYEVENTF_UNICODE),
        input(0, 'K' as u16, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
        input(VK_RETURN, 0, 0),
        input(VK_RETURN, 0, KEYEVENTF_KEYUP),
    ];

    // SendInput 自体には送信先指定がなく、この確認との間の競合は残る。
    if unsafe { GetTickCount() }.wrapping_sub(request.time) > 250
        || modifiers_down()
        || foreground() != Some(request.target)
    {
        return;
    }
    let sent = unsafe {
        SendInput(
            events.len() as u32,
            events.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent != events.len() as u32 {
        update(|state| state.enabled = false);
        tray(NIM_MODIFY);
        error("入力の送信に失敗したため Iris を無効にしました。\n一部だけ入力された可能性があります。入力欄とキー状態を確認してください。自動再送はしません。");
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
                update(|state| {
                    state.enabled = !state.enabled;
                    state.pending = None;
                });
                tray(NIM_MODIFY);
            }
            EXIT => {
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
            PostQuitMessage(1);
        }
        return 0;
    }

    match message {
        WM_SEND => send_pending(),
        WM_REFRESH | WM_TIMER => refresh(),
        WM_TRAY if lparam as u32 == WM_RBUTTONUP || lparam as u32 == WM_LBUTTONUP => menu(window),
        WM_CLOSE => PostQuitMessage(0),
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
