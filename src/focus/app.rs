use crate::{
    diagnostic::{self, App, Context, Trace},
    intervention::{self, Events, Kind, Observation, Truth},
    logic::{Decision, F1State},
    policy, uia,
};
use std::{
    cell::RefCell,
    ptr::null_mut,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{LibraryLoader::GetModuleHandleW, SystemInformation::GetTickCount, Threading::*},
    UI::{Accessibility::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

static EVENTS: Events = Events::EMPTY;
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
    pub context: Context,
    pub window: Window,
    pub epoch: u64,
    pub start_tick: u32,
    pub started: Instant,
    pub register: bool,
}
struct State {
    target: Option<(Window, App)>,
    keys: [F1State; 2],
    sender: SyncSender<Request>,
    active: Option<Request>,
    next_id: u64,
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
pub fn identify(window: Window) -> Option<App> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, window.pid);
        if process.is_null() {
            return None;
        }
        let mut buffer = [0u16; 32768];
        let mut length = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) != 0;
        CloseHandle(process);
        ok.then(|| App::from_path(&String::from_utf16_lossy(&buffer[..length as usize])))
            .flatten()
    }
}

/// 修飾付きのキーを試作の操作として扱わない。
fn modifiers() -> bool {
    [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|&key| unsafe { GetAsyncKeyState(key as i32) } < 0)
}

/// 失敗理由と、判定時に実際に観測した世代を保持する。
pub struct CheckFailure {
    pub reason: &'static str,
    pub epoch: u64,
}

/// 判定に実際に用いた世代を失敗理由と一緒に返す。
pub fn check(request: Request) -> Result<(), CheckFailure> {
    let same_window = foreground() == Some(request.window);
    let epoch = EVENTS.epoch();
    let same_epoch = epoch == request.epoch;
    let elapsed = request.started.elapsed().as_millis();
    let result = if STOP.load(Ordering::SeqCst) {
        Err("stopped")
    } else if !policy::valid(same_window, same_epoch, elapsed) {
        Err(if !same_window {
            "foreground_changed"
        } else if !same_epoch {
            "input_or_foreground_intervention"
        } else {
            "deadline_exceeded"
        })
    } else if modifiers() {
        Err("modifier_down")
    } else {
        Ok(())
    };
    result.map_err(|reason| CheckFailure { reason, epoch })
}

/// 要求開始との差を観測済みの世代まで記録し、後着イベントを原因に混ぜない。
pub fn trace_interventions(
    request: Request,
    trace: &mut Trace,
    point: &'static str,
    end: Option<u64>,
) {
    let end = end.unwrap_or_else(|| EVENTS.epoch());
    let snapshot = EVENTS.snapshot(request.epoch, end);
    let mut counts = [0u64; intervention::KIND_COUNT];
    let mut injected = 0;
    for event in &snapshot.events {
        counts[event.observation.kind as usize] += 1;
        if event.observation.injected == Truth::True {
            injected += 1;
        }
    }
    trace.record(format_args!("intervention_point={point} start_epoch={} observed_epoch={end} epoch_delta={:?} missing={} counts_complete={} detail_omitted={}", request.epoch, snapshot.total, snapshot.missing, snapshot.total.is_some() && snapshot.missing == 0, snapshot.events.len().saturating_sub(8)));
    trace.record(format_args!("intervention_point={point} foreground={} mouse_move={} mouse_button={} mouse_wheel={} mouse_other={} keyboard_other_or_injected={} new_F1_F2_request={} watchdog={} injected={injected}", counts[0], counts[1], counts[2], counts[3], counts[4], counts[5], counts[6], counts[7]));
    for event in snapshot
        .events
        .iter()
        .skip(snapshot.events.len().saturating_sub(8))
    {
        let observation = event.observation;
        let same_request_window = if observation.kind == Kind::Foreground && observation.window != 0
        {
            Truth::known(observation.window == request.window.hwnd)
        } else {
            Truth::Unknown
        };
        let same_request_process =
            if observation.kind == Kind::Foreground && observation.window_pid != 0 {
                Truth::known(observation.window_pid == request.window.pid)
            } else {
                Truth::Unknown
            };
        let injected = if matches!(observation.kind, Kind::Foreground | Kind::Watchdog) {
            "not_applicable"
        } else {
            observation.injected.label()
        };
        trace.record(format_args!("intervention_point={point} event_epoch={} kind={} injected={injected} received_mono_us={:?} event_tick_delta_ms={} received_tick_delta_ms={} delivery_tick_delta_ms={} tick_clock=coarse same_request_window={} same_request_process={} same_received_foreground={} attribution=unknown", event.epoch, observation.kind.label(), diagnostic::relative_mono_us(observation.received_mono_ns, diagnostic::mono_ns(request.started)), intervention::relative_tick(observation.event_tick, request.start_tick), intervention::relative_tick(observation.received_tick, request.start_tick), observation.received_tick.wrapping_sub(observation.event_tick), same_request_window.label(), same_request_process.label(), observation.same_received_foreground.label()));
    }
}

/// フック外の周期処理で対象候補を更新する。
fn refresh() {
    let current = foreground();
    let target = current
        .and_then(|w| identify(w).map(|app| (w, app)))
        .filter(|&(_, app)| !crate::DISCOVERY_ONLY || app == App::ChatGpt)
        .filter(|_| foreground() == current);
    STATE.with(|s| {
        let mut borrow = s.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return;
        };
        state.target = target;

        let expired = state
            .active
            .is_some_and(|request| request.started.elapsed().as_millis() >= policy::BUDGET_MS);
        if BUSY.load(Ordering::SeqCst) && !state.timed_out && expired {
            state.timed_out = true;
            STOP.store(true, Ordering::SeqCst);
            let tick = unsafe { GetTickCount() };
            let received = Instant::now();
            EVENTS.record(|| observe(Kind::Watchdog, Truth::Unknown, tick, received));
            if let Some(request) = state.active {
                eprintln!(
                    "{}",
                    request.context.line(
                        "watchdog",
                        format_args!(
                            "result=timeout restart_required=true late_focus_possible=true"
                        )
                    )
                );
            }
        }
    });
}

/// 時計と既知の分類だけを記録し、前面通知以外の所属情報は不明にする。
fn observe(kind: Kind, injected: Truth, event_tick: u32, received: Instant) -> Observation {
    Observation {
        kind,
        injected,
        event_tick,
        received_tick: unsafe { GetTickCount() },
        received_mono_ns: diagnostic::mono_ns(received),
        window: 0,
        window_pid: 0,
        same_received_foreground: Truth::Unknown,
    }
}

/// 先に失効させ、受信時の有限な所属照会を診断専用として付ける。
unsafe extern "system" fn foreground_event(
    _: HWINEVENTHOOK,
    _: u32,
    window: HWND,
    _: i32,
    _: i32,
    _: u32,
    event_tick: u32,
) {
    let received = Instant::now();
    EVENTS.record(|| {
        let mut observation = observe(Kind::Foreground, Truth::Unknown, event_tick, received);
        observation.window = window as usize;
        if !window.is_null() {
            let before = GetForegroundWindow();
            let mut pid = 0;
            let tid = GetWindowThreadProcessId(window, &mut pid);
            let after = GetForegroundWindow();
            if tid != 0 && pid != 0 {
                observation.window_pid = pid;
                if !before.is_null() && before == after {
                    observation.same_received_foreground = Truth::known(window == before);
                }
            }
        }
        observation
    });
    STATE.with(|s| {
        if let Some(state) = s.borrow_mut().as_mut() {
            state.target = None;
        }
    });
}

/// マウス操作の内容を記録せず、介入だけを数える。
unsafe extern "system" fn mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let received = Instant::now();
        // 座標やボタン値をコピーせず、種別・注入フラグ・時刻だけを読む。
        let data = lparam as *const MSLLHOOKSTRUCT;
        let kind = match wparam as u32 {
            WM_MOUSEMOVE => Kind::MouseMove,
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
            | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP => Kind::MouseButton,
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => Kind::MouseWheel,
            _ => Kind::MouseOther,
        };
        EVENTS.record(|| {
            observe(
                kind,
                Truth::known((*data).flags & LLMHF_INJECTED != 0),
                (*data).time,
                received,
            )
        });
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

/// F1/F2 の最初の物理押下だけを有界ワーカーへ渡す。
unsafe extern "system" fn keyboard(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    let received = Instant::now();
    let key = &*(lparam as *const KBDLLHOOKSTRUCT);
    let index = if key.vkCode == VK_F1 as u32 {
        Some(0)
    } else if !crate::DISCOVERY_ONLY && key.vkCode == VK_F2 as u32 {
        Some(1)
    } else {
        None
    };
    if key.flags & LLKHF_INJECTED != 0 || index.is_none() {
        EVENTS.record(|| {
            observe(
                Kind::KeyboardOther,
                Truth::known(key.flags & LLKHF_INJECTED != 0),
                key.time,
                received,
            )
        });
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
    let swallowed = STATE.with(|s| {
        let mut borrow = s.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return false;
        };
        let target = state.target.filter(|&(w, _)| foreground() == Some(w));
        let eligible = target.is_some()
            && !STOP.load(Ordering::SeqCst)
            && !modifiers()
            && key.flags & LLKHF_ALTDOWN == 0;
        let decision = state.keys[index.unwrap()].event(down, eligible, false);
        if decision == Decision::Send {
            // 別の操作キーも前の処理を中止し、処理中には予約を追加しない。
            let start_tick = GetTickCount();
            let epoch =
                EVENTS.record(|| observe(Kind::NewShortcut, Truth::False, key.time, received));
            if !BUSY.swap(true, Ordering::SeqCst) {
                let (window, app) = target.unwrap();
                let started = received;
                let context = Context {
                    id: state.next_id,
                    register: index == Some(1),
                    app,
                    started,
                };
                state.next_id += 1;
                let request = Request {
                    context,
                    window,
                    epoch,
                    start_tick,
                    started,
                    register: index == Some(1),
                };
                state.active = Some(request);
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
            if crate::PATH_ONLY {
                println!("req=0 key=- app=- stage=ready ms=0 F1=inspect_path ChatGPT_only=true metadata_strings_visible=true review_before_sharing=true no_focus_text_enter_ime_writes=true");
            } else if crate::DISCOVERY_ONLY {
                println!("req=0 key=- app=- stage=ready ms=0 F1=inspect ChatGPT_only=true no_focus_text_enter_ime_writes=true");
            } else {
                println!("req=0 key=- app=- stage=ready ms=0 F2=register F1=focus no_text_enter_ime_writes=true");
            }
            while let Ok(request) = receiver.recv() {
                let report = session.execute(request);
                println!("{report}");
                BUSY.store(false, Ordering::SeqCst);
            }
        }
        Err(_) => {
            eprintln!("req=0 key=- app=- stage=init ms=0 result=failed restart_required=true");
            STOP.store(true, Ordering::SeqCst);
        }
    }
}

/// コンソールを開いて操作を待ち、Enter による終了時にフックを外す。
pub fn run() {
    diagnostic::initialize_clock();
    if crate::PATH_ONLY {
        println!("req=0 key=- app=- stage=start ms=0 build=path-readonly-v1 stop_normal_iris_first=true no_uncommitted_ime_text=true local_display_only=true review_before_sharing=true console_Enter=exit");
    } else if crate::DISCOVERY_ONLY {
        println!("req=0 key=- app=- stage=start ms=0 build=discovery-readonly-v2 stop_normal_iris_first=true no_uncommitted_ime_text=true console_Enter=exit");
    } else {
        println!("req=0 key=- app=- stage=start ms=0 build=focus-diagnostics-v4 stop_normal_iris_first=true no_uncommitted_ime_text=true console_Enter=exit");
    }
    unsafe {
        let class: Vec<u16> = "Iris.Tray.Window".encode_utf16().chain(Some(0)).collect();
        if !FindWindowW(class.as_ptr(), null_mut()).is_null() {
            eprintln!("req=0 key=- app=- stage=init ms=0 result=another_iris_running");
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
            eprintln!("req=0 key=- app=- stage=init ms=0 result=window_registration_failed");
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
            eprintln!("req=0 key=- app=- stage=init ms=0 result=guard_window_failed");
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
                next_id: 1,
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
            eprintln!("req=0 key=- app=- stage=init ms=0 result=hook_timer_failed");
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
