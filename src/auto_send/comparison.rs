use crate::{
    model::{self, Backend, ImePhase, InputReply, Outcome},
    types::ImeReply,
};

/// IME操作を呼ばず、既存Writerの送信検証を通して一回だけ投入する。
pub fn no_ime(backend: &mut impl Backend) -> Outcome {
    let mut out = Outcome {
        status: "comparison_no_input; ime_untouched",
        manual: false,
        input: None,
        restore: None,
    };
    if backend.closing() || !backend.guarded() {
        return out;
    }
    let input = backend.send_once();
    out.input = Some(input);
    if !input.attempted {
        return out;
    }
    if input.inserted != model::INPUT_COUNT {
        out.status = "input_partial_or_failed; no_retry; restore_skipped";
        out.manual = true;
    } else if !backend.guarded() {
        out.status = "guard_changed_after_input; manual_check";
        out.manual = true;
    } else {
        out.status = "input_enqueued; ime_untouched; app_and_tsf_result_unknown";
    }
    out
}

/// 比較版だけに観測を挟み、通常の期限・送信ガードは内側へ委譲する。
pub struct Observed<'a, B: Backend> {
    pub inner: B,
    pub sample: &'a mut dyn FnMut(ImePhase, ImeReply) -> bool,
    pub failed: bool,
}
impl<B: Backend> Backend for Observed<'_, B> {
    /// 一度観測中に失効した要求を再開しない。
    fn guarded(&mut self) -> bool {
        !self.failed && self.inner.guarded()
    }
    /// 終了時の既存動作を維持する。
    fn closing(&self) -> bool {
        self.failed || self.inner.closing()
    }
    /// 開閉を元の経路で取得する。
    fn read_open(&mut self) -> ImeReply {
        self.inner.read_open()
    }
    /// 変更要求を追加せず委譲する。
    fn set_open(&mut self, open: bool) -> ImeReply {
        self.inner.set_open(open)
    }
    /// Writerの一回制約を維持する。
    fn send_once(&mut self) -> InputReply {
        self.inner.send_once()
    }
    /// 既存の復元前待機を維持する。
    fn wait_before_restore(&mut self) {
        self.inner.wait_before_restore();
    }
    /// 復元用UIAとWin32ガードを維持する。
    fn before_restore(&mut self) -> bool {
        !self.failed && self.inner.before_restore()
    }
    /// 元のガード証拠を返す。
    fn guard_evidence(&self) -> model::guard::Evidence {
        self.inner.guard_evidence()
    }
    /// 既に得たopen値を記録し、追加の読取り前後も元のガードを確認する。
    fn observe_phase(&mut self, phase: ImePhase, open: ImeReply) {
        if !self.guarded() || !(self.sample)(phase, open) || !self.inner.guarded() {
            self.failed = true;
        }
    }
}

/// 不明な開閉値をオン・オフへ読み替えない。
pub fn open_label(reply: ImeReply) -> String {
    match reply {
        ImeReply::Raw(0) => "0".into(),
        ImeReply::Raw(1) => "1".into(),
        _ => format!("unknown({reply:?})"),
    }
}
/// conversionの生値は未検証の観測値としてだけ表示する。
pub fn conversion_label(reply: ImeReply) -> String {
    match reply {
        ImeReply::Raw(v) => format!("0x{v:X}"),
        _ => format!("unknown({reply:?})"),
    }
}
/// 応答の取得完了と、実際の日本語モードの成功を分離する。
pub fn raw_reads_complete(valid: bool, open: ImeReply, conversion: ImeReply) -> bool {
    valid && matches!(open, ImeReply::Raw(0 | 1)) && matches!(conversion, ImeReply::Raw(_))
}

pub const OFFSETS: [u32; 3] = [100, 300, 1000];
pub const OBSERVATION_BUDGET_MS: u32 = 1200;
/// 後追い経路には読取り・時計・待機だけを渡し、書込み能力を持たせない。
pub trait Timeline {
    /// 復元直後からの経過時間を返す。
    fn elapsed(&self) -> u32;
    /// 元の履歴と保存先を確認する。
    fn check(&mut self) -> bool;
    /// 短い読取り待機を行う。
    fn wait(&mut self);
    /// 指定オフセットの状態を一回記録する。
    fn sample(&mut self, offset: u32) -> bool;
}
/// 復元直後からの絶対オフセットで一回ずつ観測し、失効後は再開しない。
pub fn follow(t: &mut impl Timeline) -> bool {
    for offset in OFFSETS {
        loop {
            if t.elapsed() > OBSERVATION_BUDGET_MS || !t.check() {
                return false;
            }
            if t.elapsed() >= offset {
                break;
            }
            t.wait();
        }
        if !t.sample(offset) || t.elapsed() > OBSERVATION_BUDGET_MS || !t.check() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Fake {
        replies: VecDeque<ImeReply>,
        calls: Vec<&'static str>,
        valid: bool,
        inserted: u32,
        closing: bool,
    }
    impl Fake {
        /// 元オンから復元までの応答列を作る。
        fn new() -> Self {
            Self {
                replies: [1, 0, 0, 1].map(ImeReply::Raw).into(),
                calls: vec![],
                valid: true,
                inserted: 6,
                closing: false,
            }
        }
    }
    impl Backend for Fake {
        /// 模擬した介入状態を返す。
        fn guarded(&mut self) -> bool {
            self.valid
        }
        /// 模擬した終了状態を返す。
        fn closing(&self) -> bool {
            self.closing
        }
        /// 開閉読取りの回数と応答を記録する。
        fn read_open(&mut self) -> ImeReply {
            self.calls.push("read");
            self.replies.pop_front().unwrap()
        }
        /// 追加のIME変更がないことを確認できるよう記録する。
        fn set_open(&mut self, open: bool) -> ImeReply {
            self.calls.push(if open { "on" } else { "off" });
            ImeReply::Raw(0)
        }
        /// 一括投入数を模擬する。
        fn send_once(&mut self) -> InputReply {
            self.calls.push("input");
            InputReply {
                attempted: true,
                inserted: self.inserted,
            }
        }
        /// 待機要求の有無だけを記録する。
        fn wait_before_restore(&mut self) {
            self.calls.push("wait");
        }
    }
    /// 非操作方式では成功・部分投入・中止のすべてでIME APIを呼ばない。
    #[test]
    fn no_ime_never_calls_ime_or_wait() {
        for inserted in 0..=6 {
            let mut b = Fake::new();
            b.inserted = inserted;
            let out = no_ime(&mut b);
            assert_eq!(b.calls, ["input"]);
            assert_eq!(out.manual, inserted != 6);
        }
        for closing in [false, true] {
            let mut b = Fake::new();
            b.valid = false;
            b.closing = closing;
            assert!(no_ime(&mut b).input.is_none());
            assert!(b.calls.is_empty());
        }
    }
    /// 相の順序と観測境界の失効を、本番モデルへ接続して確認する。
    #[test]
    fn phase_order_and_failures() {
        for fail in [
            None,
            Some(ImePhase::Start),
            Some(ImePhase::Off),
            Some(ImePhase::Restored),
        ] {
            let mut phases = vec![];
            let mut capture = |p, _| {
                phases.push(p);
                Some(p) != fail
            };
            let mut b = Observed {
                inner: Fake::new(),
                sample: &mut capture,
                failed: false,
            };
            let result = model::run(&mut b);
            if fail.is_none() {
                assert!(!result.manual);
                assert_eq!(
                    b.inner.calls,
                    ["read", "off", "read", "input", "wait", "read", "on", "read"]
                );
            }
            if fail == Some(ImePhase::Start) {
                assert_eq!(b.inner.calls, ["read"]);
            }
            if fail == Some(ImePhase::Off) {
                assert!(!b.inner.calls.contains(&"input"));
                assert!(!b.inner.calls.contains(&"on"));
            }
            if fail == Some(ImePhase::Restored) {
                assert!(result.manual);
            }
            drop(b);
            if fail.is_none() {
                assert_eq!(phases, [ImePhase::Start, ImePhase::Off, ImePhase::Restored]);
            }
        }
    }
    /// 元オフ・未知・部分投入では復元観測や復元要求を追加しない。
    #[test]
    fn observation_does_not_create_restoration_paths() {
        for case in 0..3 {
            let mut inner = Fake::new();
            if case == 0 {
                inner.replies = [ImeReply::Raw(0), ImeReply::Raw(0)].into();
            }
            if case == 1 {
                inner.replies = [ImeReply::Failed(0)].into();
            }
            if case == 2 {
                inner.inserted = 3;
            }
            let mut phases = vec![];
            let mut sample = |p, _| {
                phases.push(p);
                true
            };
            let mut b = Observed {
                inner,
                sample: &mut sample,
                failed: false,
            };
            let out = model::run(&mut b);
            assert!(!b.inner.calls.contains(&"on"));
            if case < 2 {
                assert!(!b.inner.calls.contains(&"off"));
            }
            if case == 1 {
                assert!(out.input.is_none());
            }
            if case == 2 {
                assert!(out.manual);
            }
            drop(b);
            assert!(!phases.contains(&ImePhase::Restored));
        }
    }

    /// WriterのUIA拒否・介入・二重呼出しでも非操作方式からIME操作や再送を起こさない。
    #[test]
    fn no_ime_keeps_writer_guards_and_single_send() {
        use crate::writer::{ValidationStage, Writer};
        for allowed in [false, true] {
            let mut stages = vec![];
            let mut validate = |s| {
                stages.push(s);
                allowed
            };
            let mut b = Writer {
                inner: Fake::new(),
                quick: &|| true,
                validate: &mut validate,
                failed: false,
                consumed: false,
                evidence: model::guard::Evidence::UNKNOWN,
            };
            let first = no_ime(&mut b);
            let second = no_ime(&mut b);
            assert_eq!(first.input.unwrap().attempted, allowed);
            assert!(!second.input.is_some_and(|r| r.attempted));
            assert_eq!(b.inner.calls, if allowed { vec!["input"] } else { vec![] });
            drop(b);
            assert_eq!(stages, [ValidationStage::Send]);
        }
        let mut validate = |_| panic!("intervention must stop before UIA");
        let mut b = Writer {
            inner: Fake::new(),
            quick: &|| false,
            validate: &mut validate,
            failed: false,
            consumed: false,
            evidence: model::guard::Evidence::UNKNOWN,
        };
        assert!(no_ime(&mut b).input.is_none());
        assert!(b.inner.calls.is_empty());
    }
    /// 未取得・失敗・不正open値を成功値に変換しない。conversionの0は生値だけ。
    #[test]
    fn unknown_measurements_remain_unknown() {
        for value in [ImeReply::NotQueried, ImeReply::Failed(0), ImeReply::Raw(2)] {
            assert!(open_label(value).starts_with("unknown"));
            assert!(!raw_reads_complete(true, value, ImeReply::Raw(0)));
        }
        for value in [ImeReply::NotQueried, ImeReply::Failed(0)] {
            assert!(conversion_label(value).starts_with("unknown"));
            assert!(!raw_reads_complete(true, ImeReply::Raw(1), value));
        }
        assert_eq!(conversion_label(ImeReply::Raw(0)), "0x0");
        assert!(!raw_reads_complete(
            false,
            ImeReply::Raw(1),
            ImeReply::Raw(25)
        ));
        assert!(raw_reads_complete(
            true,
            ImeReply::Raw(1),
            ImeReply::Raw(25)
        ));
    }

    struct Clock {
        at: u32,
        stop: u32,
        samples: Vec<u32>,
        cost: u32,
    }
    impl Timeline for Clock {
        /// 模擬時刻を返す。
        fn elapsed(&self) -> u32 {
            self.at
        }
        /// 指定時刻以降の介入を再現する。
        fn check(&mut self) -> bool {
            self.at < self.stop
        }
        /// 短い待機を時刻だけで再現する。
        fn wait(&mut self) {
            self.at += 5;
        }
        /// 読取りにかかる時間と呼出しを記録する。
        fn sample(&mut self, o: u32) -> bool {
            self.samples.push(o);
            self.at += self.cost;
            true
        }
    }
    /// 絶対時刻・遅い応答・途中介入・期限切れを再現し、追試や書込みをしない。
    #[test]
    fn bounded_read_only_timeline() {
        let mut t = Clock {
            at: 0,
            stop: u32::MAX,
            samples: vec![],
            cost: 20,
        };
        assert!(follow(&mut t));
        assert_eq!(t.samples, OFFSETS);
        assert_eq!(t.at, 1020);
        for stop in [0, 99, 100, 200, 300, 999, 1000, 1010] {
            let mut t = Clock {
                at: 0,
                stop,
                samples: vec![],
                cost: 20,
            };
            assert!(!follow(&mut t));
            assert!(t.samples.iter().all(|o| *o < stop));
        }
        let mut t = Clock {
            at: 0,
            stop: u32::MAX,
            samples: vec![],
            cost: 1200,
        };
        assert!(!follow(&mut t));
        assert_eq!(t.samples, [100]);
    }
}
