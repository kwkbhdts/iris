use crate::model::guard::{Evidence, Reason};
use crate::{
    model::{self, Backend, InputReply},
    types::ImeReply,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationStage {
    Send,
    Restore,
}

/// 既存IMEバックエンドへ固定世代ガードと送信直前のUIA検証を接続する。
pub struct Writer<'a, B: Backend> {
    pub inner: B,
    pub quick: &'a dyn Fn() -> bool,
    pub validate: &'a mut dyn FnMut(ValidationStage) -> bool,
    pub failed: bool,
    pub consumed: bool,
    pub evidence: Evidence,
}
impl<B: Backend> Backend for Writer<'_, B> {
    /// 一度の検証失敗を保持し、同じ世代に見えても再開しない。
    fn guarded(&mut self) -> bool {
        if self.failed {
            return false;
        }
        if !(self.quick)() {
            self.evidence = self.inner.guard_evidence();
            self.evidence.current = Reason::ExternalGuard;
            self.failed = true;
        } else if !self.inner.guarded() {
            self.evidence = self.inner.guard_evidence();
            self.failed = true;
        } else if !(self.quick)() {
            self.evidence = self.inner.guard_evidence();
            self.evidence.current = Reason::ExternalGuard;
            self.failed = true;
        }
        !self.failed
    }
    /// 終了や期限切れでは新しいIMEオフ・入力を開始しない。
    fn closing(&self) -> bool {
        !(self.quick)() || self.inner.closing()
    }
    /// 保存したIMEへの既存の開閉問い合わせを使う。
    fn read_open(&mut self) -> ImeReply {
        self.inner.read_open()
    }
    /// 保存したIMEへの既存の開閉要求を使う。
    fn set_open(&mut self, open: bool) -> ImeReply {
        self.inner.set_open(open)
    }
    /// 入力直前に対象を再検証し、通った場合だけ選択済み固定列を一括送信する。
    fn send_once(&mut self) -> InputReply {
        if self.consumed || self.closing() || !self.guarded() {
            return InputReply {
                attempted: false,
                inserted: 0,
            };
        }
        self.consumed = true;
        if !(self.validate)(ValidationStage::Send) {
            self.evidence = self.inner.guard_evidence();
            self.evidence.current = Reason::ExternalGuard;
            self.failed = true;
        }
        if !self.guarded() {
            return InputReply {
                attempted: false,
                inserted: 0,
            };
        }
        self.inner.send_once()
    }
    /// 復元前の既存100ms待機中にも、新しい世代と元の期限を確認する。
    fn wait_before_restore(&mut self) {
        if !model::wait_before_restore(self) {
            self.evidence.wait = Some(self.evidence.current);
        }
    }
    /// 送信後のスクロールや画面更新でも、復元対象をUIAで再確認する。
    fn before_restore(&mut self) -> bool {
        if !self.guarded() {
            return false;
        }
        if !(self.validate)(ValidationStage::Restore) {
            self.evidence = self.inner.guard_evidence();
            self.evidence.current = Reason::ExternalGuard;
            self.failed = true;
        }
        self.guarded()
    }
    /// 内側の診断を委譲し、外側の失敗と待機中の原因も保持する。
    fn guard_evidence(&self) -> Evidence {
        if self.failed {
            self.evidence
        } else {
            self.inner.guard_evidence()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    struct Fake<'a> {
        active: &'a Cell<bool>,
        on: bool,
        abort_after_off: bool,
        sends: usize,
        inserted: u32,
    }
    impl Backend for Fake<'_> {
        fn guarded(&mut self) -> bool {
            self.active.get()
        }
        fn closing(&self) -> bool {
            false
        }
        fn read_open(&mut self) -> ImeReply {
            ImeReply::Raw(usize::from(self.on))
        }
        fn set_open(&mut self, open: bool) -> ImeReply {
            self.on = open;
            if !open && self.abort_after_off {
                self.active.set(false);
            }
            ImeReply::Raw(0)
        }
        fn send_once(&mut self) -> InputReply {
            self.sends += 1;
            InputReply {
                attempted: true,
                inserted: self.inserted,
            }
        }
        fn wait_before_restore(&mut self) {}
    }
    /// IMEオフ後の介入や送信直前の候補差し替えでは、一度も投入しない。
    #[test]
    fn intervention_after_ime_and_candidate_change_block_input() {
        for (abort_after_off, candidate_same) in [(true, true), (false, false)] {
            let active = Cell::new(true);
            let quick = || active.get();
            let mut validate = |_| candidate_same;
            let inner = Fake {
                active: &active,
                on: true,
                abort_after_off,
                sends: 0,
                inserted: 6,
            };
            let mut writer = Writer {
                inner,
                quick: &quick,
                validate: &mut validate,
                failed: false,
                consumed: false,
                evidence: Evidence::UNKNOWN,
            };
            let result = model::run(&mut writer);
            assert_eq!(writer.inner.sends, 0);
            assert!(result.manual);
        }
    }
    /// 検証中の介入は最終ガードで止まり、元に戻っても再開しない。
    #[test]
    fn intervention_during_validation_latches_failure() {
        let active = Cell::new(true);
        let quick = || active.get();
        let mut validate = |_| {
            active.set(false);
            true
        };
        let inner = Fake {
            active: &active,
            on: false,
            abort_after_off: false,
            sends: 0,
            inserted: 6,
        };
        let mut writer = Writer {
            inner,
            quick: &quick,
            validate: &mut validate,
            failed: false,
            consumed: false,
            evidence: Evidence::UNKNOWN,
        };
        assert!(!writer.send_once().attempted);
        active.set(true);
        assert!(!writer.send_once().attempted);
        assert_eq!(writer.inner.sends, 0);
    }
    /// 部分投入でモデルもアダプターも再送せず、完全投入にも二回目はない。
    #[test]
    fn partial_and_complete_input_are_each_attempted_once() {
        for inserted in [0, 3, 6] {
            let active = Cell::new(true);
            let quick = || active.get();
            let mut validate = |_| true;
            let inner = Fake {
                active: &active,
                on: false,
                abort_after_off: false,
                sends: 0,
                inserted,
            };
            let mut writer = Writer {
                inner,
                quick: &quick,
                validate: &mut validate,
                failed: false,
                consumed: false,
                evidence: Evidence::UNKNOWN,
            };
            let result = model::run(&mut writer);
            assert_eq!(result.input.unwrap().inserted, inserted);
            assert_eq!(result.manual, inserted != 6);
            assert!(!writer.send_once().attempted);
            assert_eq!(writer.inner.sends, 1);
        }
    }
}

#[cfg(test)]
mod regression {
    use super::*;
    use crate::intervention::{Events, Kind, Observation, Truth};
    use std::{cell::Cell, collections::VecDeque};

    /// 入力内容を持たない履歴を本番と同じ記録器へ渡す。
    fn record(events: &Events, kind: Kind, injected: Truth) -> u64 {
        events.record(|| Observation {
            kind,
            injected,
            event_tick: 0,
            received_tick: 0,
            received_mono_ns: 0,
            window: 0,
            window_pid: 0,
            same_received_foreground: Truth::Unknown,
        })
    }

    struct Script<'a> {
        events: &'a Events,
        reads: VecDeque<ImeReply>,
        writes: VecDeque<ImeReply>,
        calls: Vec<&'static str>,
        after: Option<(Kind, Truth)>,
        focus: &'a Cell<bool>,
        lose_focus: bool,
        inserted: u32,
    }
    impl Backend for Script<'_> {
        fn guarded(&mut self) -> bool {
            self.focus.get()
        }
        fn closing(&self) -> bool {
            false
        }
        fn read_open(&mut self) -> ImeReply {
            record(self.events, Kind::MouseWheel, Truth::False);
            self.reads.pop_front().expect("unexpected IME read")
        }
        fn set_open(&mut self, open: bool) -> ImeReply {
            self.calls.push(if open { "on" } else { "off" });
            record(self.events, Kind::MouseMove, Truth::False);
            self.writes.pop_front().expect("unexpected IME write")
        }
        fn send_once(&mut self) -> InputReply {
            self.calls.push("input");
            if let Some((kind, injected)) = self.after {
                record(self.events, kind, injected);
            }
            for _ in 0..1000 {
                record(self.events, Kind::MouseMove, Truth::False);
            }
            if self.lose_focus {
                self.focus.set(false);
            }
            InputReply {
                attempted: true,
                inserted: self.inserted,
            }
        }
        fn wait_before_restore(&mut self) {
            panic!("outer guard must remain active");
        }
        fn guard_evidence(&self) -> Evidence {
            Evidence {
                current: if self.focus.get() {
                    Reason::Unknown
                } else {
                    Reason::FocusChanged
                },
                pointer_allowed: true,
                ..Evidence::UNKNOWN
            }
        }
    }

    /// 探索・IME・送信後の多数移動でも、同じF1基準と検証済み通知だけを使う。
    #[test]
    fn auto_path_keeps_fixed_history_through_send_and_restore() {
        for transition in [false, true] {
            let events = Events::EMPTY;
            let start = record(&events, Kind::NewShortcut, Truth::False);
            for _ in 0..1000 {
                record(&events, Kind::MouseHorizontalWheel, Truth::False);
            }
            assert!(events.check(start, None).permits());
            let permitted = transition.then(|| {
                [
                    record(&events, Kind::Foreground, Truth::Unknown),
                    record(&events, Kind::Foreground, Truth::Unknown),
                ]
            });
            let focus = Cell::new(true);
            let quick = || events.check(start, permitted).permits();
            let mut validate = |_| {
                record(&events, Kind::MouseHorizontalWheel, Truth::False);
                true
            };
            let inner = Script {
                events: &events,
                reads: [1, 0, 0, 1].map(ImeReply::Raw).into(),
                writes: [0, 0].map(ImeReply::Raw).into(),
                calls: vec![],
                after: None,
                focus: &focus,
                lose_focus: false,
                inserted: 6,
            };
            let mut writer = Writer {
                inner,
                quick: &quick,
                validate: &mut validate,
                failed: false,
                consumed: false,
                evidence: Evidence::UNKNOWN,
            };
            let result = model::run(&mut writer);
            assert!(!result.manual);
            assert_eq!(writer.inner.calls, ["off", "input", "on"]);
            assert!(writer.guard_evidence().pointer_allowed);
            assert!(!writer.send_once().attempted);
            assert_eq!(start, 1);
        }
    }

    /// 通知例外があってもキー・注入・未知・フォーカスの介入を保持する。
    #[test]
    fn auto_restore_latches_intervention_and_keeps_evidence() {
        for (kind, injected) in [
            (Kind::MouseButton, Truth::False),
            (Kind::MouseWheel, Truth::True),
            (Kind::MouseHorizontalWheel, Truth::Unknown),
            (Kind::KeyboardOther, Truth::False),
            (Kind::MouseMove, Truth::True),
            (Kind::MouseMove, Truth::Unknown),
            (Kind::Foreground, Truth::Unknown),
        ] {
            let events = Events::EMPTY;
            let start = record(&events, Kind::NewShortcut, Truth::False);
            let permitted = Some([
                record(&events, Kind::Foreground, Truth::Unknown),
                record(&events, Kind::Foreground, Truth::Unknown),
            ]);
            let focus = Cell::new(true);
            let quick = || events.check(start, permitted).permits();
            let mut validate = |_| true;
            let inner = Script {
                events: &events,
                reads: [1, 0].map(ImeReply::Raw).into(),
                writes: [0].map(ImeReply::Raw).into(),
                calls: vec![],
                after: Some((kind, injected)),
                focus: &focus,
                lose_focus: false,
                inserted: 6,
            };
            let mut writer = Writer {
                inner,
                quick: &quick,
                validate: &mut validate,
                failed: false,
                consumed: false,
                evidence: Evidence::UNKNOWN,
            };
            let result = model::run(&mut writer);
            assert!(result.manual);
            assert_eq!(writer.inner.calls, ["off", "input"]);
            let evidence = result.restore.unwrap().evidence;
            assert_eq!(evidence.current, Reason::ExternalGuard);
            assert_eq!(evidence.wait, Some(Reason::ExternalGuard));
            assert!(!writer.guarded());
        }
    }

    /// 部分投入・読取り失敗・復元失敗・内側のフォーカス変更でも再送しない。
    #[test]
    fn wrapped_model_preserves_failure_paths() {
        for case in 0..7 {
            let events = Events::EMPTY;
            let start = record(&events, Kind::NewShortcut, Truth::False);
            let focus = Cell::new(true);
            let quick = || events.check(start, None).permits();
            let mut validate = |_| true;
            let (reads, writes, expected, code) = match case {
                0 => (vec![ImeReply::Failed(5)], vec![], vec![], "IRIS-I101"),
                1 => (
                    vec![ImeReply::Raw(1), ImeReply::Failed(5)],
                    vec![ImeReply::Raw(0)],
                    vec!["off"],
                    "IRIS-I103",
                ),
                2 => (
                    vec![ImeReply::Raw(1), ImeReply::Raw(0)],
                    vec![ImeReply::Raw(0)],
                    vec!["off", "input"],
                    "IRIS-I104",
                ),
                3 => (
                    vec![ImeReply::Raw(1), ImeReply::Raw(0), ImeReply::Failed(5)],
                    vec![ImeReply::Raw(0)],
                    vec!["off", "input"],
                    "IRIS-I106",
                ),
                4 => (
                    vec![ImeReply::Raw(1), ImeReply::Raw(0), ImeReply::Raw(0)],
                    vec![ImeReply::Raw(0), ImeReply::Failed(5)],
                    vec!["off", "input", "on"],
                    "IRIS-I107",
                ),
                5 => (
                    vec![ImeReply::Raw(1), ImeReply::Raw(0)],
                    vec![ImeReply::Raw(0)],
                    vec!["off", "input"],
                    "IRIS-I106",
                ),
                _ => (
                    vec![
                        ImeReply::Raw(1),
                        ImeReply::Raw(0),
                        ImeReply::Raw(0),
                        ImeReply::Raw(0),
                    ],
                    vec![ImeReply::Raw(0), ImeReply::Raw(0)],
                    vec!["off", "input", "on"],
                    "IRIS-I108",
                ),
            };
            let inner = Script {
                events: &events,
                reads: reads.into(),
                writes: writes.into(),
                calls: vec![],
                after: None,
                focus: &focus,
                lose_focus: case == 5,
                inserted: if case == 2 { 3 } else { 6 },
            };
            let mut writer = Writer {
                inner,
                quick: &quick,
                validate: &mut validate,
                failed: false,
                consumed: false,
                evidence: Evidence::UNKNOWN,
            };
            let result = model::run(&mut writer);
            assert_eq!(result.diagnostic(), code);
            assert_eq!(writer.inner.calls, expected);
            if case == 5 {
                let evidence = result.restore.unwrap().evidence;
                assert_eq!(evidence.current, Reason::FocusChanged);
                assert_eq!(evidence.wait, Some(Reason::FocusChanged));
                focus.set(true);
                assert!(!writer.guarded());
            }
        }
    }
    /// スクロール自体を許可しても、候補の差替え・不可視・無効・複数化・フォーカス変更は投入しない。
    #[test]
    fn scroll_requires_a_still_valid_target_before_input() {
        for wheel in [Kind::MouseWheel, Kind::MouseHorizontalWheel] {
            for changed in 0..=7 {
                let events = Events::EMPTY;
                let start = record(&events, Kind::NewShortcut, Truth::False);
                let focus = Cell::new(true);
                let quick = || events.check(start, None).permits();
                // UIA応答を模擬し、実際の属性・候補数の判定関数を通す。
                let mut validate = |_| {
                    record(&events, wheel, Truth::False);
                    crate::selection::tests::revalidate_case(changed)
                };
                let inner = Script {
                    events: &events,
                    reads: [1, 0, 0, 1].map(ImeReply::Raw).into(),
                    writes: [0, 0].map(ImeReply::Raw).into(),
                    calls: vec![],
                    after: None,
                    focus: &focus,
                    lose_focus: false,
                    inserted: 6,
                };
                let mut writer = Writer {
                    inner,
                    quick: &quick,
                    validate: &mut validate,
                    failed: false,
                    consumed: false,
                    evidence: Evidence::UNKNOWN,
                };
                let result = model::run(&mut writer);
                if changed == 0 {
                    assert!(!result.manual);
                    assert_eq!(writer.inner.calls, ["off", "input", "on"]);
                } else {
                    assert!(result.manual);
                    assert_eq!(writer.inner.calls, ["off"]);
                    assert!(!writer.send_once().attempted);
                    assert_eq!(
                        result.restore.unwrap().evidence.current,
                        Reason::ExternalGuard
                    );
                }
                assert_eq!(start, 1);
            }
        }
    }
    /// UIA再生成でも保存IME操作先が一致すれば一回復元し、不一致なら停止する。
    #[test]
    fn restore_uses_saved_ime_context_after_input() {
        for wheel in [Kind::MouseWheel, Kind::MouseHorizontalWheel] {
            for changed in crate::restore::tests::CASES {
                let events = Events::EMPTY;
                let start = record(&events, Kind::NewShortcut, Truth::False);
                let focus = Cell::new(true);
                let quick = || events.check(start, None).permits();
                let mut stages = vec![];
                let mut validate = |stage| {
                    stages.push(stage);
                    if stage == ValidationStage::Send {
                        return true;
                    }
                    record(&events, wheel, Truth::False);
                    let valid = crate::restore::tests::validate_case(changed);
                    if !valid {
                        // 追加診断が全一致を返しても、元の拒否結果を維持する。
                        assert!(crate::failure::tests::matching_sample().complete);
                    }
                    valid
                };
                let inner = Script {
                    events: &events,
                    reads: [1, 0, 0, 1].map(ImeReply::Raw).into(),
                    writes: [0, 0].map(ImeReply::Raw).into(),
                    calls: vec![],
                    after: Some((wheel, Truth::False)),
                    focus: &focus,
                    lose_focus: false,
                    inserted: 6,
                };
                let mut writer = Writer {
                    inner,
                    quick: &quick,
                    validate: &mut validate,
                    failed: false,
                    consumed: false,
                    evidence: Evidence::UNKNOWN,
                };
                let result = model::run(&mut writer);
                if matches!(
                    changed,
                    crate::restore::tests::Case::Unchanged
                        | crate::restore::tests::Case::ReplacedUia
                ) {
                    assert!(!result.manual);
                    assert_eq!(writer.inner.calls, ["off", "input", "on"]);
                    assert_eq!(result.diagnostic(), "IRIS-I000");
                } else {
                    assert!(result.manual);
                    assert_eq!(writer.inner.calls, ["off", "input"]);
                    assert_eq!(result.diagnostic(), "IRIS-I106");
                }
                assert!(!writer.send_once().attempted);
                assert_eq!(stages, [ValidationStage::Send, ValidationStage::Restore]);
            }
        }
    }
}
