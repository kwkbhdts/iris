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

pub const ENTER_DELAY_MS: u32 = 100;
const SEND_DEADLINE_MS: u32 = 250;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Queued,
    Text,
    Waiting { since: u32 },
    Enter,
}

#[derive(Clone, Copy)]
struct SendRequest<T> {
    id: usize,
    target: T,
    created: u32,
    phase: Phase,
}

#[derive(Clone, Copy)]
pub struct PendingSend<T> {
    next_id: usize,
    request: Option<SendRequest<T>>,
}

impl<T: Copy> PendingSend<T> {
    // 1 は前面確認タイマー用。取消後も識別子を再利用しない。
    pub const EMPTY: Self = Self {
        next_id: 2,
        request: None,
    };

    /// 保留は一件だけ受け付け、処理中の連打では置き換えない。
    pub fn reserve(&mut self, target: T, now: u32) -> Option<usize> {
        if self.request.is_some() {
            return None;
        }
        let id = self.next_id;
        self.next_id = id.checked_add(1)?;
        self.request = Some(SendRequest {
            id,
            target,
            created: now,
            phase: Phase::Queued,
        });
        Some(id)
    }

    /// 現在の予約と識別子が一致するときだけ対象を返す。
    pub fn target(&self, id: usize) -> Option<T> {
        self.request.filter(|r| r.id == id).map(|r| r.target)
    }

    /// 予約を取り消し、停止すべきタイマーの識別子を返す。
    pub fn cancel(&mut self) -> Option<usize> {
        self.request.take().map(|r| r.id)
    }

    /// 文字送信を一度だけ開始し、条件不一致や期限切れなら取り消す。
    pub fn begin_text(&mut self, id: usize, now: u32, allowed: bool) -> bool {
        let Some(request) = self.request.as_mut().filter(|r| r.id == id) else {
            return false;
        };
        if request.phase != Phase::Queued {
            return false;
        }
        if !allowed || now.wrapping_sub(request.created) > SEND_DEADLINE_MS {
            self.cancel();
            return false;
        }
        request.phase = Phase::Text;
        true
    }

    /// 全文字の送信成功後から Enter の待機時間を計る。
    pub fn text_sent(&mut self, id: usize, now: u32) -> bool {
        let Some(request) = self.request.as_mut().filter(|r| r.id == id) else {
            return false;
        };
        if request.phase != Phase::Text {
            return false;
        }
        if now.wrapping_sub(request.created) > SEND_DEADLINE_MS {
            self.cancel();
            return false;
        }
        request.phase = Phase::Waiting { since: now };
        true
    }

    /// 100ms 経過後の Enter を一度だけ許可し、古い通知は無視する。
    pub fn begin_enter(&mut self, id: usize, now: u32, allowed: bool) -> bool {
        let Some(request) = self.request.as_mut().filter(|r| r.id == id) else {
            return false;
        };
        let Phase::Waiting { since } = request.phase else {
            return false;
        };
        if !allowed || now.wrapping_sub(request.created) > SEND_DEADLINE_MS {
            self.cancel();
            return false;
        }
        if now.wrapping_sub(since) < ENTER_DELAY_MS {
            return false;
        }
        request.phase = Phase::Enter;
        true
    }

    /// 終了した予約だけを片付け、別の新しい予約には触れない。
    pub fn finish(&mut self, id: usize) {
        if self.target(id).is_some() {
            self.cancel();
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

    /// 文字送信から 100ms を待ち、タイマーが重複しても Enter は一度だけ。
    #[test]
    fn enter_waits_from_text_completion_and_runs_once() {
        let mut pending = PendingSend::EMPTY;
        let id = pending.reserve(42, 0).unwrap();
        assert!(!pending.begin_enter(id, 100, true));
        assert!(pending.begin_text(id, 10, true));
        assert!(!pending.begin_text(id, 10, true));
        assert!(!pending.begin_enter(id, 110, true));
        assert!(pending.text_sent(id, 20));
        assert!(!pending.text_sent(id, 30));
        assert!(!pending.begin_enter(id, 119, true));
        assert!(pending.begin_enter(id, 120, true));
        assert!(!pending.begin_enter(id, 121, true));
        pending.finish(id);
        assert!(!pending.begin_enter(id, 122, true));
    }

    /// 待機や送信のどの段階でも、連打は元の一件を置き換えない。
    #[test]
    fn repeated_requests_do_not_overlap() {
        let mut pending = PendingSend::EMPTY;
        let id = pending.reserve(42, 0).unwrap();
        assert_eq!(pending.reserve(99, 1), None);
        assert!(pending.begin_text(id, 2, true));
        assert_eq!(pending.reserve(99, 3), None);
        assert!(pending.text_sent(id, 4));
        assert_eq!(pending.reserve(99, 5), None);
        assert!(pending.begin_enter(id, 104, true));
        assert_eq!(pending.reserve(99, 105), None);
        assert_eq!(pending.target(id), Some(42));
        pending.finish(id);
        assert!(pending.reserve(99, 106).is_some());
    }

    /// 取消済みの送信通知とタイマー通知は新しい予約に影響しない。
    #[test]
    fn stale_messages_cannot_send_or_cancel_a_new_request() {
        let mut pending = PendingSend::EMPTY;
        let old = pending.reserve(42, 0).unwrap();
        assert!(pending.begin_text(old, 0, true));
        assert!(pending.text_sent(old, 1));
        assert_eq!(pending.cancel(), Some(old));
        let new = pending.reserve(42, 50).unwrap();
        assert_ne!(old, new);
        assert!(!pending.begin_text(old, 60, true));
        assert!(pending.begin_text(new, 60, true));
        assert!(pending.text_sent(new, 61));
        assert!(!pending.begin_enter(old, 200, true));
        pending.finish(old);
        assert_eq!(pending.target(new), Some(42));
        assert!(pending.begin_enter(new, 200, true));
    }

    /// 前面・有効状態・修飾キーの条件不一致は予約を取り消す。
    #[test]
    fn failed_guard_cancels_without_later_resumption() {
        let mut pending = PendingSend::EMPTY;
        let first = pending.reserve(42, 0).unwrap();
        assert!(!pending.begin_text(first, 1, false));
        assert_eq!(pending.target(first), None);

        let second = pending.reserve(42, 10).unwrap();
        assert!(pending.begin_text(second, 10, true));
        assert!(pending.text_sent(second, 11));
        assert!(!pending.begin_enter(second, 111, false));
        assert_eq!(pending.target(second), None);
        assert!(!pending.begin_enter(second, 112, true));
    }

    /// F1 から 250ms を超えた文字送信・待機移行・Enter を拒否する。
    #[test]
    fn deadline_applies_to_both_sends() {
        let mut pending = PendingSend::EMPTY;
        let id = pending.reserve(42, 0).unwrap();
        assert!(!pending.begin_text(id, 251, true));
        let id = pending.reserve(42, 0).unwrap();
        assert!(pending.begin_text(id, 1, true));
        assert!(!pending.text_sent(id, 251));
        let id = pending.reserve(42, 0).unwrap();
        assert!(pending.begin_text(id, 1, true));
        assert!(pending.text_sent(id, 2));
        assert!(!pending.begin_enter(id, 251, true));
        assert_eq!(pending.target(id), None);
    }

    /// 文字送信中の取消後に古い完了通知が来ても Enter を予約しない。
    #[test]
    fn cancellation_during_text_send_survives_reentry() {
        let mut pending = PendingSend::EMPTY;
        let old = pending.reserve(42, 0).unwrap();
        assert!(pending.begin_text(old, 1, true));
        pending.cancel();
        let new = pending.reserve(99, 2).unwrap();
        assert!(!pending.text_sent(old, 3));
        pending.finish(old);
        assert_eq!(pending.target(new), Some(99));
        assert!(pending.begin_text(new, 4, true));
    }

    /// GetTickCount が周回しても 100ms の待機と期限を比較できる。
    #[test]
    fn timing_handles_tick_count_wraparound() {
        let mut pending = PendingSend::EMPTY;
        let start = u32::MAX - 50;
        let id = pending.reserve(42, start).unwrap();
        assert!(pending.begin_text(id, start, true));
        assert!(pending.text_sent(id, start.wrapping_add(10)));
        assert!(!pending.begin_enter(id, start.wrapping_add(109), true));
        assert!(pending.begin_enter(id, start.wrapping_add(110), true));
    }

    /// 待機中の F1 は長押しでも押し直しでも追加予約を作らない。
    #[test]
    fn f1_repeats_during_wait_do_not_queue_another_enter() {
        let mut key = F1State::EMPTY;
        let mut pending = PendingSend::EMPTY;
        assert_eq!(key.event(true, true, false), Decision::Send);
        let id = pending.reserve(42, 0).unwrap();
        assert!(pending.begin_text(id, 0, true));
        assert!(pending.text_sent(id, 1));
        assert_eq!(key.event(true, true, false), Decision::Swallow);
        assert_eq!(key.event(false, true, false), Decision::Swallow);
        assert_eq!(key.event(true, true, false), Decision::Send);
        assert_eq!(pending.reserve(42, 2), None);
        assert!(pending.begin_enter(id, 101, true));
        pending.finish(id);
        assert_eq!(key.event(true, true, false), Decision::Swallow);
        assert_eq!(key.event(false, true, false), Decision::Swallow);
    }
}
