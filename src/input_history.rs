use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Unchanged,
    PointerOnly,
    AllowedTransition,
    BlockingEvent,
    Unknown,
}

impl Check {
    /// 送信する両版で処理開始から復元まで同じ移動・ホイール限定ポリシーを使う。
    pub fn permits(self) -> bool {
        matches!(
            self,
            Self::Unchanged | Self::PointerOnly | Self::AllowedTransition
        )
    }
}

/// F1以降の移動以外を、後続の移動で隠さないための単一書込側履歴。
pub struct History {
    published: AtomicU64,
    blocking: AtomicU64,
    previous_blocking: AtomicU64,
    earlier_blocking: AtomicU64,
    unknown: AtomicU64,
    overflow: AtomicBool,
}

impl History {
    pub const EMPTY: Self = Self {
        published: AtomicU64::new(0),
        blocking: AtomicU64::new(0),
        previous_blocking: AtomicU64::new(0),
        earlier_blocking: AtomicU64::new(0),
        unknown: AtomicU64::new(0),
        overflow: AtomicBool::new(false),
    };

    /// UIスレッドで世代加算後に発行する。欠落・逆行・周回は許容しない。
    pub fn record(&self, epoch: u64, allowed_pointer: bool) {
        let previous = self.published.load(Ordering::SeqCst);
        if epoch <= previous {
            self.overflow.store(true, Ordering::SeqCst);
        }
        if previous.checked_add(1) != Some(epoch) {
            self.unknown.store(epoch, Ordering::SeqCst);
        }
        if !allowed_pointer {
            self.earlier_blocking.store(
                self.previous_blocking.load(Ordering::SeqCst),
                Ordering::SeqCst,
            );
            self.previous_blocking
                .store(self.blocking.load(Ordering::SeqCst), Ordering::SeqCst);
            self.blocking.store(epoch, Ordering::SeqCst);
        }
        self.published.store(epoch, Ordering::SeqCst);
    }

    /// F1時点の固定基準から照合する。発行中や変化中は再試行せず不明にする。
    pub fn check(&self, start: u64, observed: u64, epoch: &AtomicU64) -> Check {
        self.check_with_transition(start, observed, epoch, None)
    }

    /// F1基準を動かさず、別途検証済みの二通知だけを例外にする。
    pub fn check_with_transition(
        &self,
        start: u64,
        observed: u64,
        epoch: &AtomicU64,
        permitted: Option<[u64; 2]>,
    ) -> Check {
        let before = self.published.load(Ordering::SeqCst);
        let blocking = self.blocking.load(Ordering::SeqCst);
        let previous = self.previous_blocking.load(Ordering::SeqCst);
        let earlier = self.earlier_blocking.load(Ordering::SeqCst);
        let unknown = self.unknown.load(Ordering::SeqCst);
        let overflow = self.overflow.load(Ordering::SeqCst);
        let after = self.published.load(Ordering::SeqCst);
        if overflow
            || observed < start
            || before != observed
            || after != observed
            || blocking > observed
            || unknown > observed
            || epoch.load(Ordering::SeqCst) != observed
            || unknown > start
        {
            return Check::Unknown;
        }
        if let Some([first, last]) = permitted {
            return if start < first
                && first < last
                && last <= observed
                && previous == first
                && blocking == last
                && earlier <= start
            {
                Check::AllowedTransition
            } else {
                Check::BlockingEvent
            };
        }
        if blocking > start {
            return Check::BlockingEvent;
        }
        if observed == start {
            Check::Unchanged
        } else {
            Check::PointerOnly
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerEvent {
    Move,
    VerticalWheel,
    HorizontalWheel,
    Other,
}

/// 既知の非注入の移動・縦横ホイールだけを許可し、クリック等は含めない。
pub fn plain_pointer_event(event: PointerEvent, injected: bool, known: bool) -> bool {
    matches!(
        event,
        PointerEvent::Move | PointerEvent::VerticalWheel | PointerEvent::HorizontalWheel
    ) && !injected
        && known
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 許可した通知の前後にある別の介入を、多数の移動で隠さない。
    #[test]
    fn only_exact_two_notifications_can_be_excepted() {
        for extra in [None, Some(1), Some(4), Some(7)] {
            let history = History::EMPTY;
            let epoch = AtomicU64::new(0);
            for next in 1..=1000 {
                epoch.store(next, Ordering::SeqCst);
                history.record(next, next != 3 && next != 6 && Some(next) != extra);
            }
            let checked = history.check_with_transition(0, 1000, &epoch, Some([3, 6]));
            assert_eq!(checked.permits(), extra.is_none());
            assert!(!history.check(0, 1000, &epoch).permits());
            for pair in [[6, 3], [3, 7], [0, 6], [3, 1001]] {
                assert!(!history
                    .check_with_transition(0, 1000, &epoch, Some(pair))
                    .permits());
            }
        }
    }

    /// 通知例外も欠落や発行中を正常扱いしない。
    #[test]
    fn transition_does_not_cover_unknown_or_unpublished_history() {
        let history = History::EMPTY;
        let epoch = AtomicU64::new(1);
        history.record(1, false);
        epoch.store(2, Ordering::SeqCst);
        history.record(2, false);
        assert!(history
            .check_with_transition(0, 2, &epoch, Some([1, 2]))
            .permits());
        epoch.store(3, Ordering::SeqCst);
        assert_eq!(
            history.check_with_transition(0, 3, &epoch, Some([1, 2])),
            Check::Unknown
        );
        epoch.store(4, Ordering::SeqCst);
        history.record(4, true);
        assert_eq!(
            history.check_with_transition(0, 4, &epoch, Some([1, 2])),
            Check::Unknown
        );
    }

    /// 並行発行中は不明でもよいが、既知の介入を許可へ戻さない。
    #[test]
    fn concurrent_motion_never_erases_a_blocker() {
        let history = History::EMPTY;
        let epoch = AtomicU64::new(1);
        history.record(1, false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for next in 2..=5000 {
                    epoch.store(next, Ordering::SeqCst);
                    history.record(next, true);
                }
            });
            for _ in 0..5000 {
                assert!(!history
                    .check(0, epoch.load(Ordering::SeqCst), &epoch)
                    .permits());
            }
        });
        assert_eq!(history.check(0, 5000, &epoch), Check::BlockingEvent);
    }
}
