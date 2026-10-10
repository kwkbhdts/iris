#[derive(Clone, Copy, Default)]
pub struct F1State {
    pub down: bool,
    swallowed: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Pass,
    Swallow,
    Send,
}

impl F1State {
    pub const EMPTY: Self = Self {
        down: false,
        swallowed: false,
    };

    /// 最初の押下だけで動作を決め、キーを離すまで維持する。
    pub fn event(&mut self, down: bool, eligible: bool, injected: bool) -> Decision {
        if injected {
            return Decision::Pass;
        }

        if !down {
            let swallowed = self.swallowed;
            *self = Self::default();
            return if swallowed {
                Decision::Swallow
            } else {
                Decision::Pass
            };
        }

        if self.down {
            return if self.swallowed {
                Decision::Swallow
            } else {
                Decision::Pass
            };
        }

        self.down = true;
        self.swallowed = eligible;
        if eligible {
            Decision::Send
        } else {
            Decision::Pass
        }
    }
}

/// パス末尾の実行ファイル名だけを大文字小文字を無視して照合する。
pub fn allowed_image(path: &str) -> bool {
    let name = path.rsplit(['\\', '/']).next().unwrap_or_default();
    ["Claude.exe", "firefox.exe", "chrome.exe", "ChatGPT.exe"]
        .iter()
        .any(|expected| name.eq_ignore_ascii_case(expected))
}

#[derive(Clone, Copy)]
pub struct Pending<T> {
    next_id: usize,
    request: Option<(usize, T, bool)>,
}

impl<T: Copy> Pending<T> {
    pub const EMPTY: Self = Self {
        next_id: 1,
        request: None,
    };

    /// 処理中・取消処理中も一件を保持し、新しい処理を重ねない。
    pub fn reserve(&mut self, value: T) -> Option<usize> {
        if self.request.is_some() {
            return None;
        }
        let id = self.next_id;
        self.next_id = id.checked_add(1)?;
        self.request = Some((id, value, false));
        Some(id)
    }

    /// メッセージが重複しても作業スレッドへ一度だけ渡す。
    pub fn start(&mut self, id: usize) -> Option<T> {
        let (current, value, started) = self.request.as_mut()?;
        if *current != id || *started {
            return None;
        }
        *started = true;
        Some(*value)
    }

    /// 取消後も実際の完了通知まで予約を保持する。
    pub fn current(&self) -> Option<(usize, T)> {
        self.request.map(|(id, value, _)| (id, value))
    }

    /// 同じ処理の完了だけで予約を解放し、古い通知は無視する。
    pub fn finish(&mut self, id: usize) -> bool {
        if self.request.is_some_and(|(current, _, _)| current == id) {
            self.request = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 長押しは一回だけ送り、キーを離した後に再送できる。
    #[test]
    fn held_key_sends_once() {
        let mut state = F1State::default();
        assert_eq!(state.event(true, true, false), Decision::Send);
        for _ in 0..20 {
            assert_eq!(state.event(true, true, false), Decision::Swallow);
        }
        assert_eq!(state.event(false, true, false), Decision::Swallow);
        assert_eq!(state.event(true, true, false), Decision::Send);
    }

    /// 対象外や修飾付きで押し始めたキーは途中で奪わない。
    #[test]
    fn passed_press_stays_passed() {
        let mut state = F1State::default();
        assert_eq!(state.event(true, false, false), Decision::Pass);
        assert_eq!(state.event(true, true, false), Decision::Pass);
        assert_eq!(state.event(false, true, false), Decision::Pass);
    }

    /// 抑止した押下のキーアップは無効化や前面変更後も抑止する。
    #[test]
    fn swallowed_press_stays_swallowed() {
        let mut state = F1State::default();
        assert_eq!(state.event(true, true, false), Decision::Send);
        assert_eq!(state.event(true, false, false), Decision::Swallow);
        assert_eq!(state.event(false, false, false), Decision::Swallow);
        assert_eq!(state.event(true, false, false), Decision::Pass);
    }

    /// 注入イベントは押下状態にも影響させない。
    #[test]
    fn injected_events_do_not_change_state() {
        let mut state = F1State::default();
        assert_eq!(state.event(true, true, true), Decision::Pass);
        assert_eq!(state.event(true, true, false), Decision::Send);
        assert_eq!(state.event(false, true, true), Decision::Pass);
        assert_eq!(state.event(true, true, false), Decision::Swallow);
        assert_eq!(state.event(false, true, false), Decision::Swallow);
    }

    /// 類似名や別の拡張子を対象にしない。
    #[test]
    fn only_exact_executable_names_match() {
        for path in [
            r"C:\Apps\Claude.exe",
            r"C:\Firefox\FIREFOX.EXE",
            "claude.EXE",
            "chrome.exe",
            r"C:\Chrome\CHROME.EXE",
            "ChatGPT.exe",
            r"C:\Apps\CHATGPT.EXE",
        ] {
            assert!(allowed_image(path));
        }
        for path in [
            "",
            "Claude.exe.bak",
            "myfirefox.exe",
            "firefox",
            "chrome.exe.bak",
            "mychrome.exe",
            "chrome",
            "msedge.exe",
            "codex.exe",
            "ChatGPT.exe.bak",
            "myChatGPT.exe",
            "ChatGPT",
        ] {
            assert!(!allowed_image(path));
        }
    }

    /// 起動時から押されていた F1 は、離して押し直すまで送信しない。
    #[test]
    fn already_held_at_startup_passes() {
        let mut state = F1State {
            down: true,
            ..F1State::EMPTY
        };
        assert_eq!(state.event(true, true, false), Decision::Pass);
        assert_eq!(state.event(false, true, false), Decision::Pass);
        assert_eq!(state.event(true, true, false), Decision::Send);
    }

    /// 処理を渡した後も完了するまでは次を予約できない。
    #[test]
    fn pending_blocks_repeated_and_cancelled_work_until_completion() {
        let mut p = Pending::EMPTY;
        let id = p.reserve(42).unwrap();
        assert_eq!(p.reserve(99), None);
        assert_eq!(p.start(id), Some(42));
        assert_eq!(p.start(id), None);
        assert_eq!(p.reserve(99), None);
        assert!(p.finish(id));
        assert!(p.reserve(99).is_some());
    }

    /// 古い通知では新しい処理を開始・解放しない。
    #[test]
    fn stale_messages_do_not_touch_new_work() {
        let mut p = Pending::EMPTY;
        let old = p.reserve(42).unwrap();
        assert!(p.finish(old));
        let new = p.reserve(99).unwrap();
        assert_ne!(old, new);
        assert_eq!(p.start(old), None);
        assert!(!p.finish(old));
        assert_eq!(p.current(), Some((new, 99)));
        assert_eq!(p.start(new), Some(99));
    }

    /// 対象外・無効・修飾付きの最初の F1 は保留中でも透過する。
    #[test]
    fn ineligible_press_passes_while_an_operation_is_pending() {
        let mut p = Pending::EMPTY;
        p.reserve(42).unwrap();
        let mut key = F1State::EMPTY;
        assert_eq!(key.event(true, false, false), Decision::Pass);
        assert_eq!(key.event(false, false, false), Decision::Pass);
        assert_eq!(key.event(true, true, false), Decision::Send);
        assert_eq!(p.reserve(99), None);
        assert_eq!(key.event(true, true, false), Decision::Swallow);
    }
}
