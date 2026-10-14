use crate::intervention::{Kind, Snapshot, Truth};

/// 二通知の実測パターンだけを許可し、自己誘発の因果は主張しない。
pub fn allowed(snapshot: &Snapshot, pid: u32, hwnd: usize, set_start: u64, set_end: u64) -> bool {
    let events: Vec<_> = snapshot
        .events
        .iter()
        .filter(|event| !event.observation.permitted_pointer())
        .collect();
    if snapshot.missing != 0
        || snapshot.total != Some(snapshot.events.len() as u64)
        || events.len() != 2
        || pid == 0
        || hwnd == 0
        || set_start == u64::MAX
        || set_end == u64::MAX
        || set_start > set_end
    {
        return false;
    }
    let first = events[0].observation;
    let last = events[1].observation;
    events.iter().all(|e| {
        let o = e.observation;
        o.kind == Kind::Foreground
            && o.window_pid == pid
            && o.same_received_foreground == Truth::True
            && o.injected == Truth::Unknown
            && (set_start..=set_end).contains(&o.received_mono_ns)
    }) && first.window != 0
        && first.window != hwnd
        && last.window == hwnd
        && first.received_mono_ns <= last.received_mono_ns
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intervention::{Event, Observation};
    fn snapshot() -> Snapshot {
        Snapshot {
            total: Some(2),
            missing: 0,
            events: [(1, 20, 12), (2, 10, 14)]
                .into_iter()
                .map(|(epoch, window, received_mono_ns)| Event {
                    epoch,
                    observation: Observation {
                        kind: Kind::Foreground,
                        injected: Truth::Unknown,
                        event_tick: 0,
                        received_tick: 0,
                        received_mono_ns,
                        window,
                        window_pid: 7,
                        same_received_foreground: Truth::True,
                    },
                })
                .collect(),
        }
    }
    /// 同プロセスでも件数・順序・時刻・既知性のいずれかが違えば拒否する。
    #[test]
    fn only_complete_two_event_pattern_is_admitted() {
        assert!(allowed(&snapshot(), 7, 10, 11, 15));
        let mut s = snapshot();
        s.missing = 1;
        assert!(!allowed(&s, 7, 10, 11, 15));
        let mut s = snapshot();
        s.total = None;
        assert!(!allowed(&s, 7, 10, 11, 15));
        let mut s = snapshot();
        s.events.pop();
        assert!(!allowed(&s, 7, 10, 11, 15));
        let mut s = snapshot();
        s.events[0].observation.window = 10;
        assert!(!allowed(&s, 7, 10, 11, 15));
        let mut s = snapshot();
        s.events[1].observation.window = 30;
        assert!(!allowed(&s, 7, 10, 11, 15));
        for kind in [
            Kind::MouseMove,
            Kind::MouseButton,
            Kind::KeyboardOther,
            Kind::NewShortcut,
            Kind::Watchdog,
        ] {
            let mut s = snapshot();
            s.events[0].observation.kind = kind;
            assert!(!allowed(&s, 7, 10, 11, 15));
        }
        for received in [0, 10, 16, u64::MAX] {
            let mut s = snapshot();
            s.events[0].observation.received_mono_ns = received;
            assert!(!allowed(&s, 7, 10, 11, 15));
        }
        let mut s = snapshot();
        s.events[0].observation.window_pid = 8;
        assert!(!allowed(&s, 7, 10, 11, 15));
        let mut s = snapshot();
        s.events[0].observation.same_received_foreground = Truth::Unknown;
        assert!(!allowed(&s, 7, 10, 11, 15));
        assert!(!allowed(&snapshot(), 7, 10, u64::MAX, u64::MAX));
    }
    /// 二通知の周囲の既知の移動だけを除き、注入・未知・欠落は拒否する。
    #[test]
    fn movement_around_transition_keeps_strict_notification_checks() {
        for (position, kind) in (0..=2).flat_map(|p| {
            [
                Kind::MouseMove,
                Kind::MouseWheel,
                Kind::MouseHorizontalWheel,
            ]
            .map(|k| (p, k))
        }) {
            let mut s = snapshot();
            let mut movement = s.events[0];
            movement.observation.kind = kind;
            movement.observation.injected = Truth::False;
            s.events.insert(position, movement);
            for (index, event) in s.events.iter_mut().enumerate() {
                event.epoch = index as u64 + 1;
            }
            s.total = Some(3);
            assert!(allowed(&s, 7, 10, 11, 15));
            for truth in [Truth::True, Truth::Unknown] {
                s.events[position].observation.injected = truth;
                assert!(!allowed(&s, 7, 10, 11, 15));
            }
            s.events[position].observation.injected = Truth::False;
            s.missing = 1;
            assert!(!allowed(&s, 7, 10, 11, 15));
        }
    }
}
