use windows_sys::Win32::UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*};

/// 両版で同じ修飾キー集合を確認する。
pub fn modifiers_down() -> bool {
    [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|&key| unsafe { GetAsyncKeyState(key as i32) } < 0)
}

/// 不明なフラグは非注入とみなさない。
pub fn mouse_injected(flags: u32) -> Option<bool> {
    let mask = LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED;
    (flags & !mask == 0).then_some(flags & mask != 0)
}

/// 縦・横ホイールを明示的に分類し、それ以外を移動へ丸めない。
pub fn pointer_event(message: u32) -> super::input_history::PointerEvent {
    use super::input_history::PointerEvent;
    match message {
        WM_MOUSEMOVE => PointerEvent::Move,
        WM_MOUSEWHEEL => PointerEvent::VerticalWheel,
        WM_MOUSEHWHEEL => PointerEvent::HorizontalWheel,
        _ => PointerEvent::Other,
    }
}
