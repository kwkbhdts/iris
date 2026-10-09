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
    name.eq_ignore_ascii_case("Claude.exe") || name.eq_ignore_ascii_case("firefox.exe")
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
        ] {
            assert!(allowed_image(path));
        }
        for path in [
            "",
            "Claude.exe.bak",
            "myfirefox.exe",
            "firefox",
            "chrome.exe",
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
}
