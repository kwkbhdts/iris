use crate::{
    logic::{allowed_image, Decision, F1State},
    policy, uia,
};
use std::{
    cell::RefCell,
    ptr::null_mut,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{LibraryLoader::GetModuleHandleW, Threading::*},
    UI::{Accessibility::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

pub static EPOCH: AtomicU64 = AtomicU64::new(0);
pub static STOP: AtomicBool = AtomicBool::new(false);
static BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub hwnd: usize,
    pub pid: u32,
    pub tid: u32,
}
#[derive(Clone, Copy)]
pub struct Request {
    pub window: Window,
    pub epoch: u64,
    pub started: Instant,
    pub register: bool,
}
struct State {
    target: Option<Window>,
    keys: [F1State; 2],
    sender: SyncSender<Request>,
    active: Option<Instant>,
    timed_out: bool,
}
thread_local! { static STATE: RefCell<Option<State>> = const { RefCell::new(None) }; }

/// 前面ウィンドウの数値識別子だけを取得する。
pub fn foreground() -> Option<Window> {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid = 0;
        let tid = GetWindowThreadProcessId(hwnd, &mut pid);
        (!hwnd.is_null() && pid != 0 && tid != 0).then_some(Window {
            hwnd: hwnd as usize,
            pid,
            tid,
        })
    }
}

/// 対象名の照会はキーフック外で行い、パスを表示・保存しない。
pub fn allowed(window: Window) -> bool {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, window.pid);
        if process.is_null() {
            return false;
        }
        let mut buffer = [0u16; 32768];
        let mut length = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) != 0;
        CloseHandle(process);
        ok && allowed_image(&String::from_utf16_lossy(&buffer[..length as usize]))
    }
}

/// 修飾付きのキーを試作の操作として扱わない。
fn modifiers() -> bool {
    [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|&key| unsafe { GetAsyncKeyState(key as i32) } < 0)
}

/// UIA 呼び出しの間ごとに前面・介入・期限を確認する。
pub fn check(request: Request) -> Result<(), &'static str> {
    if STOP.load(Ordering::SeqCst)
        || !policy::valid(
            foreground() == Some(request.window),
            EPOCH.load(Ordering::SeqCst) == request.epoch,
            request.started.elapsed().as_millis(),
        )
        || modifiers()
    {
        Err("cancelled: foreground/input changed or deadline exceeded")
    } else {
        Ok(())
    }
}

/// フック外の周期処理で対象候補を更新する。
fn refresh() {
    let current = foreground();
    let target = current
        .filter(|&w| allowed(w))
        .filter(|_| foreground() == current);
    STATE.with(|s| {
        let mut borrow = s.borrow_mut();
        let Some(state) = borrow.as_mut() else { return; };
        state.target = target;

        let expired = state.active.is_some_and(|at| at.elapsed().as_millis() >= policy::BUDGET_MS);
        if BUSY.load(Ordering::SeqCst) && !state.timed_out && expired {
            state.timed_out = true;
            STOP.store(true, Ordering::SeqCst);
            EPOCH.fetch_add(1, Ordering::SeqCst);
            eprintln!("TIMEOUT: probe disabled. An in-flight SetFocus may finish late; close and restart.");
        }
    });
}

/// 前面切替は往復しても要求を失効させる。
unsafe extern "system" fn foreground_event(
    _: HWINEVENTHOOK,
    _: u32,
    _: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    EPOCH.fetch_add(1, Ordering::SeqCst);
    STATE.with(|s| {
        if let Some(state) = s.borrow_mut().as_mut() {
            state.target = None;
        }
    });
}

/// マウス操作の内容を記録せず、介入だけを数える。
unsafe extern "system" fn mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        EPOCH.fetch_add(1, Ordering::SeqCst);
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

/// F1/F2 の最初の物理押下だけを有界ワーカーへ渡す。
unsafe extern "system" fn keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    let key = &*(lparam as *const KBDLLHOOKSTRUCT);
    let index = if key.vkCode == VK_F1 as u32 {
        Some(0)
    } else if key.vkCode == VK_F2 as u32 {
        Some(1)
    } else {
        None
    };
    if key.flags & LLKHF_INJECTED != 0 || index.is_none() {
        EPOCH.fetch_add(1, Ordering::SeqCst);
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
    let swallowed = STATE.with(|s| {
        let mut borrow = s.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return false;
        };
        let target = state.target.filter(|&w| foreground() == Some(w));
        let eligible = target.is_some()
            && !STOP.load(Ordering::SeqCst)
            && !modifiers()
            && key.flags & LLKHF_ALTDOWN == 0;
        let decision = state.keys[index.unwrap()].event(down, eligible, false);
        if decision == Decision::Send {
            // 別の操作キーも前の処理を中止し、処理中には予約を追加しない。
            let epoch = EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
            if !BUSY.swap(true, Ordering::SeqCst) {
                let request = Request {
                    window: target.unwrap(),
                    epoch,
                    started: Instant::now(),
                    register: index == Some(1),
                };
                state.active = Some(request.started);
                if state.sender.try_send(request).is_err() {
                    BUSY.store(false, Ordering::SeqCst);
                }
            }
        }
        decision != Decision::Pass
    });
    if swallowed {
        1
    } else {
        CallNextHookEx(null_mut(), code, wparam, lparam)
    }
}

/// UIA は専用 MTA スレッド内で生成・使用・解放する。
fn worker(receiver: mpsc::Receiver<Request>) {
    match uia::Session::new() {
        Ok(mut session) => {
            println!("Ready: click the composer, F2 to register; F1 to return. No text/Enter/IME writes.");
            while let Ok(request) = receiver.recv() {
                let result = session.execute(request);
                // タイマーの次回発火より先に戻った期限切れも無効化する。
                if request.started.elapsed().as_millis() >= policy::BUDGET_MS {
                    STOP.store(true, Ordering::SeqCst);
                }
                if STOP.load(Ordering::SeqCst) {
                    eprintln!("Stopped or timed out: close and restart; an in-flight focus change cannot be undone.");
                } else {
                    match result {
                        Ok(message) => println!("{message}"),
                        Err(message) => println!("FAILED: {message}; register again with F2"),
                    }
                }
                BUSY.store(false, Ordering::SeqCst);
            }
        }
        Err(_) => {
            eprintln!("UIA initialization failed. Close this window; probe disabled.");
            STOP.store(true, Ordering::SeqCst);
        }
    }
}

/// コンソールを開いて操作を待ち、Enter による終了時にフックを外す。
pub fn run() {
    println!("Iris focus-only prototype — stop normal Iris first. No uncommitted IME text.\nPress Enter in this console to exit. Logs contain status/control type only.");
    unsafe {
        let class: Vec<u16> = "Iris.Tray.Window".encode_utf16().chain(Some(0)).collect();
        if !FindWindowW(class.as_ptr(), null_mut()).is_null() {
            eprintln!("Another Iris is running. Exit it first.");
            return;
        }
        let instance = GetModuleHandleW(null_mut());
        let definition = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        };
        if RegisterClassW(&definition) == 0 {
            eprintln!("Window registration failed.");
            return;
        }
        // 通常版にも既存 Iris と認識される隠しウィンドウを持つ。
        let guard_window = CreateWindowExW(
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
        );
        if guard_window.is_null() {
            eprintln!("Guard window creation failed.");
            return;
        }
        let mut msg: MSG = std::mem::zeroed();
        PeekMessageW(&mut msg, null_mut(), 0, 0, PM_NOREMOVE);
        let thread = GetCurrentThreadId();
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut keys = [F1State::EMPTY; 2];
        keys[0].down = GetAsyncKeyState(VK_F1 as i32) < 0;
        keys[1].down = GetAsyncKeyState(VK_F2 as i32) < 0;
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                target: None,
                keys,
                sender,
                active: None,
                timed_out: false,
            })
        });
        std::thread::spawn(move || worker(receiver));
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            STOP.store(true, Ordering::SeqCst);
            PostThreadMessageW(thread, WM_QUIT, 0, 0);
        });
        let instance = GetModuleHandleW(null_mut());
        let keyboard_hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), instance, 0);
        let mouse_hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), instance, 0);
        let event_hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            null_mut(),
            Some(foreground_event),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        let timer = SetTimer(null_mut(), 0, 100, None);
        if keyboard_hook.is_null() || mouse_hook.is_null() || event_hook.is_null() || timer == 0 {
            eprintln!("Hook/timer initialization failed.");
        } else {
            refresh();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                if msg.message == WM_TIMER {
                    refresh();
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        STOP.store(true, Ordering::SeqCst);
        UnhookWindowsHookEx(keyboard_hook);
        UnhookWindowsHookEx(mouse_hook);
        UnhookWinEvent(event_hook);
        KillTimer(null_mut(), timer);
        DestroyWindow(guard_window);
        STATE.with(|s| *s.borrow_mut() = None);
        // 応答しない COM ワーカーを無期限 join せず、プロセス終了に委ねる。
    }
}
