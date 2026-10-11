use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

const CAPACITY: usize = 128;
pub const KIND_COUNT: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Kind {
    Foreground,
    MouseMove,
    MouseButton,
    MouseWheel,
    MouseOther,
    KeyboardOther,
    NewShortcut,
    Watchdog,
}
impl Kind {
    /// 数値を検証し、不完全な記録を既知の原因に見せない。
    fn decode(value: u64) -> Option<Self> {
        let kinds: [Self; KIND_COUNT] = [
            Self::Foreground,
            Self::MouseMove,
            Self::MouseButton,
            Self::MouseWheel,
            Self::MouseOther,
            Self::KeyboardOther,
            Self::NewShortcut,
            Self::Watchdog,
        ];
        kinds.get(value as usize).copied()
    }

    /// 原因は固定ラベルのみで表示する。
    #[cfg(windows)]
    pub fn label(self) -> &'static str {
        match self {
            Self::Foreground => "foreground_event",
            Self::MouseMove => "mouse_move",
            Self::MouseButton => "mouse_button",
            Self::MouseWheel => "mouse_wheel",
            Self::MouseOther => "mouse_other",
            Self::KeyboardOther => "keyboard_other_or_injected",
            Self::NewShortcut => "new_F1_F2_request",
            Self::Watchdog => "watchdog",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub epoch: u64,
    pub kind: Kind,
    pub injected: bool,
    pub event_tick: u32,
    pub received_tick: u32,
    // 前面通知の要求ウィンドウとの照合専用。数値自体は出力しない。
    pub window: usize,
}
struct Slot {
    stamp: AtomicU64,
    kind: AtomicU64,
    injected: AtomicU64,
    ticks: AtomicU64,
    window: AtomicU64,
}
impl Slot {
    const EMPTY: Self = Self {
        stamp: AtomicU64::new(0),
        kind: AtomicU64::new(0),
        injected: AtomicU64::new(0),
        ticks: AtomicU64::new(0),
        window: AtomicU64::new(0),
    };
}

pub struct Events {
    epoch: AtomicU64,
    slots: [Slot; CAPACITY],
}
pub struct Snapshot {
    pub total: Option<u64>,
    pub missing: u64,
    pub events: Vec<Event>,
}
impl Events {
    pub const EMPTY: Self = Self {
        epoch: AtomicU64::new(0),
        slots: [const { Slot::EMPTY }; CAPACITY],
    };

    /// ガードが従来と同じ世代値を参照する。
    pub fn epoch(&self) -> u64 {
        self.epoch.load(SeqCst)
    }

    /// メッセージループの単一書き手からのみ呼ぶ。待機・割当・外部APIを使わない。
    pub fn record(
        &self,
        kind: Kind,
        injected: bool,
        event_tick: u32,
        received_tick: u32,
        window: usize,
    ) -> u64 {
        // 記録完了を待たず失効させる。読み手は書きかけを missing として扱う。
        let epoch = self.epoch.fetch_add(1, SeqCst).wrapping_add(1);
        let slot = &self.slots[epoch as usize % CAPACITY];
        slot.stamp.store(0, SeqCst);
        slot.kind.store(kind as u64, SeqCst);
        slot.injected.store(u64::from(injected), SeqCst);
        slot.ticks.store(
            (u64::from(event_tick) << 32) | u64::from(received_tick),
            SeqCst,
        );
        slot.window.store(window as u64, SeqCst);
        slot.stamp.store(epoch, SeqCst);
        epoch
    }

    /// 観測済みの世代までを有限に読む。上書き・書きかけを推測で補わない。
    pub fn snapshot(&self, start: u64, end: u64) -> Snapshot {
        let Some(total) = end.checked_sub(start) else {
            return Snapshot {
                total: None,
                missing: 0,
                events: Vec::new(),
            };
        };
        let count = total.min(CAPACITY as u64);
        let mut result = Snapshot {
            total: Some(total),
            missing: total - count,
            events: Vec::new(),
        };
        if count == 0 {
            return result;
        }
        for epoch in end - count + 1..=end {
            let slot = &self.slots[epoch as usize % CAPACITY];
            if slot.stamp.load(SeqCst) != epoch {
                result.missing += 1;
                continue;
            }
            let kind = Kind::decode(slot.kind.load(SeqCst));
            let injected = slot.injected.load(SeqCst) != 0;
            let ticks = slot.ticks.load(SeqCst);
            let window = slot.window.load(SeqCst) as usize;
            if slot.stamp.load(SeqCst) != epoch || kind.is_none() {
                result.missing += 1;
                continue;
            }
            result.events.push(Event {
                epoch,
                kind: kind.unwrap(),
                injected,
                event_tick: (ticks >> 32) as u32,
                received_tick: ticks as u32,
                window,
            });
        }
        result
    }
}

/// Win32 の32bit時刻差を要求近傍の符号付き相対値へ変換する。
pub fn relative_tick(value: u32, origin: u32) -> i32 {
    value.wrapping_sub(origin) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 要求自身の開始イベントと、後から起きたイベントを混同しない。
    #[test]
    fn exact_epoch_range_preserves_classification() {
        let log = Events::EMPTY;
        let start = log.record(Kind::NewShortcut, false, 100, 101, 0);
        let end = log.record(Kind::Foreground, false, 98, 120, 42);
        log.record(Kind::MouseMove, true, 121, 121, 0);
        let snapshot = log.snapshot(start, end);
        assert_eq!(snapshot.total, Some(1));
        assert_eq!(snapshot.missing, 0);
        let event = snapshot.events[0];
        assert_eq!(event.epoch, end);
        assert_eq!(event.kind, Kind::Foreground);
        assert!(!event.injected);
        assert_eq!(event.window, 42);
        assert_eq!(relative_tick(event.event_tick, 101), -3);
        assert_eq!(relative_tick(event.received_tick, 101), 19);
    }

    /// リング上限を超えた件数と、読み取り前の上書きを欠落として報告する。
    #[test]
    fn overwritten_records_are_not_misattributed() {
        let log = Events::EMPTY;
        for _ in 0..CAPACITY + 5 {
            log.record(Kind::MouseMove, false, 1, 1, 0);
        }
        let all = log.snapshot(0, log.epoch());
        assert_eq!(all.missing, 5);
        assert_eq!(all.events.len(), CAPACITY);
        let old = log.snapshot(0, 5);
        assert_eq!(old.missing, 5);
        assert!(old.events.is_empty());
    }

    /// 世代は進んでも公開前のスロットは原因不明として扱う。
    #[test]
    fn unpublished_slot_and_epoch_wrap_fail_closed() {
        let log = Events::EMPTY;
        log.epoch.store(1, SeqCst);
        let snapshot = log.snapshot(0, 1);
        assert_eq!(snapshot.missing, 1);
        assert!(snapshot.events.is_empty());
        assert_eq!(log.snapshot(10, 0).total, None);
        assert_eq!(relative_tick(5, u32::MAX - 4), 10);
    }
    /// 書き手と競合しても、異なる世代の属性を合成して報告しない。
    #[test]
    fn concurrent_read_keeps_published_record_consistent() {
        let log = Events::EMPTY;
        let finished = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for value in 1..=10000u32 {
                    log.record(
                        Kind::MouseMove,
                        value % 2 == 0,
                        value,
                        value,
                        value as usize,
                    );
                    if value % 64 == 0 {
                        std::thread::yield_now();
                    }
                }
                finished.store(true, SeqCst);
            });
            loop {
                let snapshot = log.snapshot(0, log.epoch());
                assert_eq!(
                    snapshot.total,
                    Some(snapshot.missing + snapshot.events.len() as u64)
                );
                for event in snapshot.events {
                    assert_eq!(event.kind, Kind::MouseMove);
                    assert_eq!(event.event_tick as u64, event.epoch);
                    assert_eq!(event.received_tick as u64, event.epoch);
                    assert_eq!(event.window as u64, event.epoch);
                    assert_eq!(event.injected, event.epoch % 2 == 0);
                }
                if finished.load(SeqCst) {
                    break;
                }
            }
        });
    }
}
