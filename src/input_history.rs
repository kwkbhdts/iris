use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Unchanged,
    MovesOnly,
    BlockingEvent,
    Unknown,
}

impl Check {
    /// 通常版は処理開始から復元まで同じ移動限定ポリシーを使う。
    pub fn permits(self) -> bool {
        matches!(self, Self::Unchanged | Self::MovesOnly)
    }
}

/// F1以降の移動以外を、後続の移動で隠さないための単一書込側履歴。
pub struct History {
    published: AtomicU64,
    blocking: AtomicU64,
    unknown: AtomicU64,
    overflow: AtomicBool,
}

impl History {
    pub const EMPTY: Self = Self {
        published: AtomicU64::new(0),
        blocking: AtomicU64::new(0),
        unknown: AtomicU64::new(0),
        overflow: AtomicBool::new(false),
    };

    /// UIスレッドで世代加算後に発行する。欠落・逆行・周回は許容しない。
    pub fn record(&self, epoch: u64, mouse_move: bool) {
        let previous = self.published.load(Ordering::SeqCst);
        if epoch <= previous {
            self.overflow.store(true, Ordering::SeqCst);
        }
        if previous.checked_add(1) != Some(epoch) {
            self.unknown.store(epoch, Ordering::SeqCst);
        }
        if !mouse_move {
            self.blocking.store(epoch, Ordering::SeqCst);
        }
        self.published.store(epoch, Ordering::SeqCst);
    }

    /// F1時点の固定基準から照合する。発行中や変化中は再試行せず不明にする。
    pub fn check(&self, start: u64, observed: u64, epoch: &AtomicU64) -> Check {
        let before = self.published.load(Ordering::SeqCst);
        let blocking = self.blocking.load(Ordering::SeqCst);
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
        if blocking > start {
            return Check::BlockingEvent;
        }
        if observed == start {
            Check::Unchanged
        } else {
            Check::MovesOnly
        }
    }
}

/// 移動メッセージと非注入を確認できた場合だけ例外候補にする。
pub fn plain_mouse_move(is_move: bool, injected: bool, known: bool) -> bool {
    is_move && !injected && known
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{self, Backend, InputReply},
        types::ImeReply,
    };

    #[derive(Clone, Copy)]
    enum Event {
        Move,
        Click,
        Wheel,
        Key,
        Focus,
        Unknown,
        Abort,
        Context,
        Expire,
    }

    struct Session {
        history: History,
        epoch: AtomicU64,
        start: u64,
        allow_moves: bool,
        aborted: bool,
        context_valid: bool,
        now: u32,
        on: bool,
        inserted: u32,
        after_input: Vec<Event>,
        calls: Vec<&'static str>,
    }

    impl Session {
        /// 元オンの入力を、Win32を呼ばない外部イベント付きモデルで表す。
        fn new(events: &[Event]) -> Self {
            Self {
                history: History::EMPTY,
                epoch: AtomicU64::new(0),
                start: 0,
                allow_moves: true,
                aborted: false,
                context_valid: true,
                now: 0,
                on: true,
                inserted: 6,
                after_input: events.to_vec(),
                calls: vec![],
            }
        }

        /// 履歴欠落や中止を含む外部イベントを、固定の開始基準とは別に進める。
        fn event(&mut self, event: Event) {
            match event {
                Event::Abort => {
                    self.aborted = true;
                    return;
                }
                Event::Context => {
                    self.context_valid = false;
                    return;
                }
                Event::Expire => {
                    self.now = model::DEADLINE_MS + 1;
                    return;
                }
                _ => {}
            }
            let next = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
            if !matches!(event, Event::Unknown) {
                self.history.record(next, matches!(event, Event::Move));
            }
        }
    }

    impl Backend for Session {
        fn guarded(&mut self) -> bool {
            if self.aborted {
                return false;
            }
            let current = self.epoch.load(Ordering::SeqCst);
            let valid = if self.allow_moves {
                self.history
                    .check(self.start, current, &self.epoch)
                    .permits()
            } else {
                current == self.start
            };
            valid && self.context_valid && model::within_deadline(0, self.now)
        }
        fn closing(&self) -> bool {
            false
        }
        fn read_open(&mut self) -> ImeReply {
            ImeReply::Raw(usize::from(self.on))
        }
        fn set_open(&mut self, open: bool) -> ImeReply {
            self.calls.push(if open { "on" } else { "off" });
            self.on = open;
            ImeReply::Raw(0)
        }
        fn send_once(&mut self) -> InputReply {
            if !self.guarded() {
                return InputReply {
                    attempted: false,
                    inserted: 0,
                };
            }
            self.calls.push("input");
            for event in std::mem::take(&mut self.after_input) {
                self.event(event);
            }
            InputReply {
                attempted: true,
                inserted: self.inserted,
            }
        }
        fn wait_before_restore(&mut self) {
            self.calls.push("wait");
            if !self.guarded() {
                self.aborted = true;
            }
        }
    }

    /// 移動だけの連続は許容候補だが、厳格な世代比較は不一致のまま。
    #[test]
    fn motion_does_not_rebase_strict_guard() {
        let history = History::EMPTY;
        let epoch = AtomicU64::new(0);
        let start = 0;
        assert_eq!(history.check(start, 0, &epoch), Check::Unchanged);
        for next in 1..=1000 {
            epoch.store(next, Ordering::SeqCst);
            history.record(next, true);
        }
        assert_ne!(epoch.load(Ordering::SeqCst), start);
        assert_eq!(history.check(start, 1000, &epoch), Check::MovesOnly);
    }

    /// クリック・ホイール・キー・フォーカス等は前後の移動で消えない。
    #[test]
    fn mixed_history_retains_blocking_event() {
        for events in [
            [false, true, true],
            [true, false, true],
            [true, true, false],
        ] {
            let history = History::EMPTY;
            let epoch = AtomicU64::new(0);
            for (index, movement) in events.into_iter().enumerate() {
                let next = index as u64 + 1;
                epoch.store(next, Ordering::SeqCst);
                history.record(next, movement);
            }
            assert_eq!(history.check(0, 3, &epoch), Check::BlockingEvent);
        }
    }

    /// 発行前・欠落・並行更新・逆行では移動だけと推測しない。
    #[test]
    fn incomplete_history_fails_closed() {
        let history = History::EMPTY;
        let epoch = AtomicU64::new(1);
        assert_eq!(history.check(0, 1, &epoch), Check::Unknown);
        history.record(1, true);
        epoch.store(2, Ordering::SeqCst);
        assert_eq!(history.check(0, 1, &epoch), Check::Unknown);
        epoch.store(3, Ordering::SeqCst);
        history.record(3, true);
        assert_eq!(history.check(0, 3, &epoch), Check::Unknown);
        assert_eq!(history.check(4, 3, &epoch), Check::Unknown);
        history.record(2, true);
        epoch.store(2, Ordering::SeqCst);
        assert_eq!(history.check(0, 2, &epoch), Check::Unknown);
    }

    /// 次のF1は新しい基準を使えるが、既存処理の基準は動かない。
    #[test]
    fn new_request_does_not_repair_old_request() {
        let history = History::EMPTY;
        let epoch = AtomicU64::new(1);
        history.record(1, false);
        let next_request = 1;
        epoch.store(2, Ordering::SeqCst);
        history.record(2, true);
        assert_eq!(history.check(0, 2, &epoch), Check::BlockingEvent);
        assert_eq!(history.check(next_request, 2, &epoch), Check::MovesOnly);
    }

    /// クリック相当・注入・分類不能は移動例外にしない。
    #[test]
    fn only_known_noninjected_motion_qualifies() {
        assert!(plain_mouse_move(true, false, true));
        for args in [
            (false, false, true),
            (true, true, true),
            (true, false, false),
        ] {
            assert!(!plain_mouse_move(args.0, args.1, args.2));
        }
    }

    /// 通常版は投入前後の移動だけなら、一回の入力と復元へ進む。
    #[test]
    fn model_allows_presend_and_postsend_motion_only() {
        let mut before = Session::new(&[]);
        before.event(Event::Move);
        model::run(&mut before);
        assert_eq!(before.calls, ["off", "input", "wait", "on"]);
        assert_eq!(before.start, 0);

        let mut after = Session::new(&[Event::Move, Event::Move]);
        let result = model::run(&mut after);
        assert!(!result.manual);
        assert_eq!(after.calls, ["off", "input", "wait", "on"]);
        assert!(after.on);
        assert_eq!(after.start, 0);
    }

    /// 投入と復元モード開始の間のクリック等も、後続移動で隠さない。
    #[test]
    fn model_blocks_mixed_events_in_both_orders() {
        for hard in [Event::Click, Event::Wheel, Event::Key, Event::Focus] {
            for events in [[Event::Move, hard], [hard, Event::Move]] {
                let mut session = Session::new(&events);
                let result = model::run(&mut session);
                assert!(result.manual);
                assert_eq!(session.calls, ["off", "input", "wait"]);
                assert!(!session.on);
                assert_eq!(session.start, 0);
            }
        }
    }

    /// 不明履歴・中止・対象不一致・期限を移動例外で解除しない。
    #[test]
    fn model_keeps_other_restore_guards_closed() {
        for event in [Event::Unknown, Event::Abort, Event::Context, Event::Expire] {
            let mut session = Session::new(&[event, Event::Move]);
            let result = model::run(&mut session);
            assert!(result.manual);
            assert_eq!(session.calls, ["off", "input", "wait"]);
            assert!(session.aborted);
            assert!(!session.on);
        }
    }

    /// 部分投入は移動だけでも復元・待機・再送に進まない。
    #[test]
    fn model_never_relaxes_partial_input() {
        for inserted in 0..model::INPUT_COUNT {
            let mut session = Session::new(&[Event::Move]);
            session.inserted = inserted;
            let result = model::run(&mut session);
            assert!(result.manual);
            assert_eq!(session.calls, ["off", "input"]);
        }
    }

    /// F1前からの移動継続でも、F1の基準を後から取り直さず一回だけ送る。
    #[test]
    fn continuing_motion_across_f1_keeps_one_fixed_snapshot() {
        let mut session = Session::new(&[Event::Move, Event::Move]);
        session.event(Event::Click);
        for _ in 0..20 {
            session.event(Event::Move);
        }
        session.start = session.epoch.load(Ordering::SeqCst); // F1で一度だけ取得。
        let captured = session.start;
        for _ in 0..20 {
            session.event(Event::Move);
        }
        let result = model::run(&mut session);
        assert!(!result.manual);
        assert_eq!(session.start, captured);
        assert_eq!(session.calls, ["off", "input", "wait", "on"]);
    }

    /// F1後の古いクリック等が後続の多数移動で隠れても、送信開始を拒否する。
    #[test]
    fn presend_mixed_or_incomplete_history_blocks_input() {
        for hard in [
            Event::Click,
            Event::Wheel,
            Event::Key,
            Event::Focus,
            Event::Unknown,
        ] {
            for hard_first in [false, true] {
                let mut session = Session::new(&[]);
                if hard_first {
                    session.event(hard);
                }
                for _ in 0..1000 {
                    session.event(Event::Move);
                }
                if !hard_first {
                    session.event(hard);
                }
                model::run(&mut session);
                assert!(session.calls.is_empty());
                assert_eq!(session.start, 0);
            }
        }
    }

    /// 移動例外を使わないバックエンドは、投入前後とも従来どおり止まる。
    #[test]
    fn strict_backend_is_not_relaxed() {
        let mut before = Session::new(&[]);
        before.allow_moves = false;
        before.event(Event::Move);
        model::run(&mut before);
        assert!(before.calls.is_empty());

        let mut after = Session::new(&[Event::Move]);
        after.allow_moves = false;
        let result = model::run(&mut after);
        assert!(result.manual);
        assert_eq!(after.calls, ["off", "input", "wait"]);
    }

    /// 世代上限の周回は、次のF1でも履歴を既知と扱わない。
    #[test]
    fn generation_overflow_stays_unknown() {
        let history = History::EMPTY;
        let epoch = AtomicU64::new(u64::MAX);
        history.record(u64::MAX, true);
        assert_eq!(history.check(0, u64::MAX, &epoch), Check::Unknown);
        epoch.store(0, Ordering::SeqCst);
        history.record(0, true);
        assert_eq!(history.check(0, 0, &epoch), Check::Unknown);
        epoch.store(1, Ordering::SeqCst);
        history.record(1, true);
        assert_eq!(history.check(1, 1, &epoch), Check::Unknown);
    }
}
