use super::types::ImeReply;

#[path = "guard.rs"]
#[allow(dead_code)]
pub mod guard;
#[cfg(windows)]
pub use crate::logic::input_history;
#[cfg(test)]
#[path = "input_history_tests.rs"]
mod input_history_tests;

#[cfg_attr(not(windows), allow(dead_code))]
pub const RESTORE_DELAY_MS: u32 = 100;
pub const DEADLINE_MS: u32 = 500;
pub const INPUT_COUNT: u32 = 6;

/// 通常はEnter1回。2回は明示比較だけで選ぶ固定入力列。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputPlan {
    SingleEnter,
    #[allow(dead_code)] // 通常トレイ版は比較列を選ばない。
    TwoEnters,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKey {
    Unicode(u16),
    Enter,
}
impl InputPlan {
    /// OKは一度だけ、各キーの押下・解放を順に一括投入する。
    pub fn events(self) -> &'static [(InputKey, bool)] {
        const EVENTS: [(InputKey, bool); 8] = [
            (InputKey::Unicode('O' as u16), false),
            (InputKey::Unicode('O' as u16), true),
            (InputKey::Unicode('K' as u16), false),
            (InputKey::Unicode('K' as u16), true),
            (InputKey::Enter, false),
            (InputKey::Enter, true),
            (InputKey::Enter, false),
            (InputKey::Enter, true),
        ];
        &EVENTS[..self.count() as usize]
    }
    /// 投入確認はモード固有の全イベント数と照合する。
    pub fn count(self) -> u32 {
        match self {
            Self::SingleEnter => INPUT_COUNT,
            Self::TwoEnters => 8,
        }
    }
}

/// 時計の周回を扱い、期限切れの入力・復元を再開させない。
pub fn within_deadline(start: u32, now: u32) -> bool {
    now.wrapping_sub(start) <= DEADLINE_MS
}

/// 自分で注入したイベントだけを介入判定から除く。本人確認には使用しない。
pub fn own_input(injected: bool, tag: usize, expected: usize) -> bool {
    injected && tag == expected
}

#[derive(Clone, Copy, Debug)]
pub struct InputReply {
    pub attempted: bool,
    pub inserted: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImePhase {
    Start,
    Off,
    Restored,
}

pub trait Backend {
    /// 比較版だけの読取り観測。通常版では追加操作をしない。
    fn observe_phase(&mut self, _phase: ImePhase, _open: ImeReply) {}

    /// 対象・介入・期限・レイアウトを再確認する。
    fn guarded(&mut self) -> bool;
    /// 終了要求後には新たな入力を送らない。
    fn closing(&self) -> bool;
    /// 従来の IME 開閉応答を取得する。
    fn read_open(&mut self) -> ImeReply;
    /// 開閉だけを設定する。
    fn set_open(&mut self, open: bool) -> ImeReply;
    /// 最後にガードを確認し、選択した固定列を待機なしで一括投入する。
    fn send_once(&mut self) -> InputReply;
    /// 入力後の復元だけの暫定待機。アプリの処理完了確認ではない。
    fn wait_before_restore(&mut self);
    /// 復元前に、版固有の対象確認も完了する。
    fn before_restore(&mut self) -> bool {
        self.guarded()
    }
    /// 再取得せず、直前の判定が残した診断だけを返す。
    fn guard_evidence(&self) -> guard::Evidence {
        guard::Evidence::UNKNOWN
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreStage {
    BeforeGuard,
    ReadUnknown,
    ReadNonzero,
    AfterGuard,
}

#[derive(Clone, Copy, Debug)]
pub struct RestoreDiagnostic {
    pub stage: RestoreStage,
    pub evidence: guard::Evidence,
    pub reply: Option<ImeReply>,
    pub input: Option<InputReply>,
}

impl RestoreDiagnostic {
    /// I106 のどの短絡分岐で止まったかを示す。
    pub fn code(self) -> &'static str {
        match self.stage {
            RestoreStage::BeforeGuard => "IRIS-I106-PRE",
            RestoreStage::ReadUnknown => "IRIS-I106-READ-UNKNOWN",
            RestoreStage::ReadNonzero => "IRIS-I106-READ-NONZERO",
            RestoreStage::AfterGuard => "IRIS-I106-POST",
        }
    }

    /// 投入数と既に得た応答だけを表示し、送信成功は主張しない。
    pub fn display(self) -> String {
        let input = match self.input {
            Some(r) if r.attempted => format!("enqueued={}/6", r.inserted),
            _ => "not_attempted".into(),
        };
        let reply = match self.reply {
            None => "not_read".into(),
            Some(ImeReply::Raw(n)) => format!("raw:{n}"),
            Some(ImeReply::Failed(n)) => format!("query_failed:{n}"),
            Some(ImeReply::NotQueried) => "not_queried".into(),
        };
        let reason = match self.stage {
            RestoreStage::ReadUnknown | RestoreStage::ReadNonzero => "not_evaluated",
            _ => self.evidence.current.code(),
        };
        format!(
            "{} guard={} wait_guard={} pointer_allowed={} saw_pointer_only={} ime={} input={} app_send_result=unknown",
            self.code(),
            reason,
            self.evidence.wait.map_or("none", guard::Reason::code),
            self.evidence.pointer_allowed,
            self.evidence.saw_pointer_only,
            reply,
            input
        )
    }
}

pub struct Outcome {
    pub status: &'static str,
    pub manual: bool,
    pub input: Option<InputReply>,
    pub restore: Option<RestoreDiagnostic>,
}

impl Outcome {
    /// 既存の停止段階を安定したコードへ写し、アプリ側の送信結果は推測しない。
    pub fn diagnostic(&self) -> &'static str {
        match self.status {
            "original_unknown; no_input_or_change" => "IRIS-I101",
            "off_request_unconfirmed; no_input" => "IRIS-I102",
            "off_not_confirmed_or_guard_failed; no_input" => "IRIS-I103",
            "input_partial_or_failed; no_retry; restore_skipped" => "IRIS-I104",
            "guard_changed_after_input; manual_check" => "IRIS-I105",
            "restore_skipped; guard_failed_or_state_unknown" => "IRIS-I106",
            "restore_request_unconfirmed" => "IRIS-I107",
            "restore_readback_unconfirmed" => "IRIS-I108",
            "input_enqueued; ime_untouched; app_and_tsf_result_unknown" => "IRIS-C000",
            "comparison_no_input; ime_untouched" => "IRIS-C101",
            "skipped_before_change"
            | "input_skipped; original_off; unchanged"
            | "input_skipped; legacy_restored; actual_tsf_state_unknown"
            | "input_enqueued; original_off; no_ime_write; app_result_unknown"
            | "input_enqueued; legacy_restored; app_and_tsf_result_unknown" => "IRIS-I000",
            _ => "IRIS-I999",
        }
    }
}

/// 復元待機中も両版で5msごとに同じガードを確認する。
#[cfg_attr(not(windows), allow(dead_code))]
pub fn wait_before_restore(backend: &mut impl Backend) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed().as_millis() < RESTORE_DELAY_MS as u128 {
        if !backend.guarded() {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    true
}

/// 元の応答0/1を分岐し、入力一回と条件付き復元だけを実行する。
pub fn run(backend: &mut impl Backend) -> Outcome {
    let mut out = Outcome {
        status: "skipped_before_change",
        manual: false,
        input: None,
        restore: None,
    };
    if backend.closing() || !backend.guarded() {
        return out;
    }
    let original = backend.read_open();
    backend.observe_phase(ImePhase::Start, original);
    let was_on = match original {
        ImeReply::Raw(0) => false,
        ImeReply::Raw(1) => true,
        _ => {
            out.status = "original_unknown; no_input_or_change";
            return out;
        }
    };
    if backend.closing() || !backend.guarded() {
        return out;
    }
    if was_on {
        out.manual = true;
        out.status = "off_request_unconfirmed; no_input";
        let off = backend.set_open(false);
        if !matches!(off, ImeReply::Raw(0)) || !backend.guarded() {
            return out;
        }
    }

    let off = backend.read_open();
    backend.observe_phase(ImePhase::Off, off);
    if !matches!(off, ImeReply::Raw(0)) || !backend.guarded() {
        out.status = "off_not_confirmed_or_guard_failed; no_input";
        return out;
    }
    if !backend.closing() {
        let input = backend.send_once();
        out.input = Some(input);
        if input.attempted && input.inserted != INPUT_COUNT {
            out.manual = true;
            out.status = "input_partial_or_failed; no_retry; restore_skipped";
            return out;
        }
    }
    let sent = out
        .input
        .is_some_and(|r| r.attempted && r.inserted == INPUT_COUNT);
    if !was_on {
        out.status = if sent {
            "input_enqueued; original_off; no_ime_write; app_result_unknown"
        } else {
            "input_skipped; original_off; unchanged"
        };
        if sent && !backend.guarded() {
            out.manual = true;
            out.status = "guard_changed_after_input; manual_check";
        }
        return out;
    }

    // 入力を実際に投入した場合だけ待つ。文字と Enter の間には待機しない。
    if sent {
        backend.wait_before_restore();
    }
    out.status = "restore_skipped; guard_failed_or_state_unknown";
    if !backend.before_restore() {
        out.restore = Some(RestoreDiagnostic {
            stage: RestoreStage::BeforeGuard,
            evidence: backend.guard_evidence(),
            reply: None,
            input: out.input,
        });
        return out;
    }
    let current = backend.read_open();
    if !matches!(current, ImeReply::Raw(0)) {
        out.restore = Some(RestoreDiagnostic {
            stage: if matches!(current, ImeReply::Raw(_)) {
                RestoreStage::ReadNonzero
            } else {
                RestoreStage::ReadUnknown
            },
            evidence: backend.guard_evidence(),
            reply: Some(current),
            input: out.input,
        });
        return out;
    }
    if !backend.guarded() {
        out.restore = Some(RestoreDiagnostic {
            stage: RestoreStage::AfterGuard,
            evidence: backend.guard_evidence(),
            reply: Some(current),
            input: out.input,
        });
        return out;
    }
    out.status = "restore_request_unconfirmed";
    let restored = backend.set_open(true);
    if !matches!(restored, ImeReply::Raw(0)) || !backend.guarded() {
        return out;
    }
    out.status = "restore_readback_unconfirmed";
    let after = backend.read_open();
    backend.observe_phase(ImePhase::Restored, after);
    if matches!(after, ImeReply::Raw(1)) && backend.guarded() {
        out.manual = false;
        out.status = if sent {
            "input_enqueued; legacy_restored; app_and_tsf_result_unknown"
        } else {
            "input_skipped; legacy_restored; actual_tsf_state_unknown"
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// 追加文字や解放漏れがなく、既存の6イベントを先頭に保つ。
    #[test]
    fn input_plans_keep_one_ok_and_bounded_enters() {
        use InputKey::{Enter, Unicode};
        let expected = [
            (Unicode(79), false),
            (Unicode(79), true),
            (Unicode(75), false),
            (Unicode(75), true),
            (Enter, false),
            (Enter, true),
        ];
        assert_eq!(InputPlan::SingleEnter.events(), expected);
        let double = InputPlan::TwoEnters.events();
        assert_eq!(&double[..6], expected);
        assert_eq!(&double[6..], [(Enter, false), (Enter, true)]);
        assert_eq!(InputPlan::SingleEnter.count(), 6);
        assert_eq!(InputPlan::TwoEnters.count(), 8);
    }

    struct Fake {
        replies: VecDeque<ImeReply>,
        events: Vec<&'static str>,
        input: InputReply,
        valid: bool,
        close: bool,
        close_after_off: bool,
        interrupt_on_wait: bool,
        fail_check: usize,
        checks: usize,
        evidence: guard::Evidence,
    }
    impl Fake {
        /// オンからの一往復を用意する。
        fn on() -> Self {
            Self {
                replies: [1, 0, 0, 0, 0, 1].map(ImeReply::Raw).into(),
                events: vec![],
                input: InputReply {
                    attempted: true,
                    inserted: INPUT_COUNT,
                },
                valid: true,
                close: false,
                close_after_off: false,
                interrupt_on_wait: false,
                fail_check: usize::MAX,
                checks: 0,
                evidence: guard::Evidence::UNKNOWN,
            }
        }
        /// 最初からオフの応答を用意する。
        fn off() -> Self {
            let mut f = Self::on();
            f.replies = [0, 0].map(ImeReply::Raw).into();
            f
        }
    }
    impl Backend for Fake {
        fn guard_evidence(&self) -> guard::Evidence {
            self.evidence
        }
        fn guarded(&mut self) -> bool {
            self.checks += 1;
            self.valid && self.checks < self.fail_check
        }
        fn closing(&self) -> bool {
            self.close
        }
        fn read_open(&mut self) -> ImeReply {
            self.replies.pop_front().expect("unexpected read")
        }
        fn set_open(&mut self, open: bool) -> ImeReply {
            self.events.push(if open { "on" } else { "off" });
            if !open && self.close_after_off {
                self.close = true;
            }
            self.read_open()
        }
        fn send_once(&mut self) -> InputReply {
            self.events.push("send");
            self.input
        }
        fn wait_before_restore(&mut self) {
            self.events.push("restore_wait");
            self.valid &= !self.interrupt_on_wait;
        }
    }

    /// 元が0なら入力一回だけで、切替・復元・待機を行わない。
    #[test]
    fn original_off_sends_without_ime_writes_or_waits() {
        let mut f = Fake::off();
        let r = run(&mut f);
        assert_eq!(f.events, ["send"]);
        assert!(!r.manual);
        assert!(r.status.contains("app_result_unknown"));
    }

    /// オフを読み取ってから一回入力し、待機は復元の前だけに置く。
    #[test]
    fn original_on_orders_off_input_restore_wait_and_on() {
        let mut f = Fake::on();
        let r = run(&mut f);
        assert_eq!(f.events, ["off", "send", "restore_wait", "on"]);
        assert!(!r.manual);
        assert!(r.status.contains("app_and_tsf_result_unknown"));
    }

    /// 最初の状態が不明なら入力も書き込みもしない。
    #[test]
    fn unknown_original_has_no_side_effects() {
        for value in [ImeReply::Raw(2), ImeReply::NotQueried, ImeReply::Failed(0)] {
            let mut f = Fake::on();
            f.replies[0] = value;
            assert!(!run(&mut f).manual);
            assert!(f.events.is_empty());
        }
    }

    /// 設定要求の失敗を未変更と断定せず、入力を止める。
    #[test]
    fn off_write_failure_stops_input() {
        let mut f = Fake::on();
        f.replies[1] = ImeReply::Failed(1460);
        assert!(run(&mut f).manual);
        assert_eq!(f.events, ["off"]);
    }

    /// オフの読み返しが不明・オンなら入力しない。
    #[test]
    fn off_readback_required_before_input() {
        for value in [ImeReply::Raw(1), ImeReply::Failed(0)] {
            let mut f = Fake::on();
            f.replies[2] = value;
            assert!(run(&mut f).manual);
            assert_eq!(f.events, ["off"]);
            let mut f = Fake::off();
            f.replies[1] = value;
            run(&mut f);
            assert!(f.events.is_empty());
        }
    }

    /// 一部投入は元の状態にかかわらず自動再送・追加 Enter・復元を止める。
    #[test]
    fn partial_input_is_never_retried() {
        for count in 0..INPUT_COUNT {
            for on in [false, true] {
                let mut f = if on { Fake::on() } else { Fake::off() };
                f.input.inserted = count;
                let r = run(&mut f);
                assert!(r.manual);
                assert_eq!(f.events.iter().filter(|&&e| e == "send").count(), 1);
                assert!(!f.events.contains(&"on"));
                assert!(!f.events.contains(&"restore_wait"));
            }
        }
    }

    /// 入力前の終了要求は入力せず、変更済みなら限定的に復元する。
    #[test]
    fn close_after_off_restores_without_sending_or_delay() {
        let mut f = Fake::on();
        f.close_after_off = true;
        let r = run(&mut f);
        assert!(!r.manual);
        assert_eq!(f.events, ["off", "on"]);
    }

    /// 最後の投入直前ガードで中止した場合も再送せず、復元だけを判定する。
    #[test]
    fn skipped_send_can_restore_without_restore_delay() {
        let mut f = Fake::on();
        f.input = InputReply {
            attempted: false,
            inserted: 0,
        };
        let r = run(&mut f);
        assert!(!r.manual);
        assert_eq!(f.events, ["off", "send", "on"]);
        assert!(r.status.starts_with("input_skipped"));
    }

    /// 初期の終了要求では何もしない。
    #[test]
    fn close_before_start_is_unchanged() {
        let mut f = Fake::on();
        f.close = true;
        run(&mut f);
        assert!(f.events.is_empty());
    }

    /// 復元待機中の対象移動・介入・期限切れでは書き戻さない。
    #[test]
    fn interruption_during_restore_wait_stops_writeback() {
        let mut f = Fake::on();
        f.interrupt_on_wait = true;
        assert!(run(&mut f).manual);
        assert_eq!(f.events, ["off", "send", "restore_wait"]);
    }

    /// 復元直前の状態が不明なら上書きしない。
    #[test]
    fn uncertain_restore_state_is_not_overwritten() {
        let mut f = Fake::on();
        f.replies[3] = ImeReply::Failed(0);
        assert!(run(&mut f).manual);
        assert!(!f.events.contains(&"on"));
    }

    /// 復元の設定・確認に失敗しても入力を繰り返さない。
    #[test]
    fn restore_failure_does_not_repeat_input() {
        for index in [4, 5] {
            let mut f = Fake::on();
            f.replies[index] = ImeReply::Failed(0);
            assert!(run(&mut f).manual);
            assert_eq!(f.events, ["off", "send", "restore_wait", "on"]);
        }
    }

    /// 再確認点で失効した場合、それ以降の入力・復元を行わない。
    #[test]
    fn guards_stop_side_effects() {
        for stop in 1..=8 {
            let mut f = Fake::on();
            f.fail_check = stop;
            let r = run(&mut f);
            if stop <= 4 {
                assert!(!f.events.contains(&"send"));
            }
            if stop <= 6 {
                assert!(!f.events.contains(&"on"));
            }
            if stop > 2 {
                assert!(r.manual);
            }
        }
    }

    /// 自分の注入タグと期限の境界を検証する。
    #[test]
    fn injected_tag_and_deadline_are_precise() {
        assert!(own_input(true, 10, 10));
        assert!(!own_input(false, 10, 10));
        assert!(!own_input(true, 11, 10));
        let start = u32::MAX - 20;
        assert!(within_deadline(start, start.wrapping_add(500)));
        assert!(!within_deadline(start, start.wrapping_add(501)));
    }

    /// 失敗を起こした段階ごとに固定コードを返し、投入・復元の呼出しは増やさない。
    #[test]
    fn diagnostics_identify_ime_stages() {
        for (index, code) in [
            (0, "IRIS-I101"),
            (1, "IRIS-I102"),
            (2, "IRIS-I103"),
            (3, "IRIS-I106"),
            (4, "IRIS-I107"),
            (5, "IRIS-I108"),
        ] {
            let mut f = Fake::on();
            f.replies[index] = ImeReply::Failed(1460);
            let result = run(&mut f);
            assert_eq!(result.diagnostic(), code);
            assert!(f.events.iter().filter(|&&e| e == "send").count() <= 1);
        }
        let mut f = Fake::on();
        f.input.inserted = 3;
        assert_eq!(run(&mut f).diagnostic(), "IRIS-I104");
    }

    /// 全入力投入後の確認不能を未送信や再送可能と扱わない。
    #[test]
    fn diagnostics_after_input_do_not_assert_send_failure() {
        let mut f = Fake::off();
        f.fail_check = 4;
        let r = run(&mut f);
        assert_eq!(r.diagnostic(), "IRIS-I105");
        assert_eq!(r.input.unwrap().inserted, INPUT_COUNT);
        assert_eq!(f.events, ["send"]);
        let mut f = Fake::on();
        f.replies[5] = ImeReply::Failed(0);
        let r = run(&mut f);
        assert_eq!(r.diagnostic(), "IRIS-I108");
        assert_eq!(r.input.unwrap().inserted, INPUT_COUNT);
        assert_eq!(f.events, ["off", "send", "restore_wait", "on"]);
    }

    /// 復元前・読取後で判定回数と問い合わせ回数を維持する。
    #[test]
    fn restore_guard_detail_preserves_short_circuit_order() {
        for (check, stage, remaining) in [
            (5, RestoreStage::BeforeGuard, 3),
            (6, RestoreStage::AfterGuard, 2),
        ] {
            let mut f = Fake::on();
            f.fail_check = check;
            f.evidence.current = guard::Reason::ForegroundChanged;
            let result = run(&mut f);
            let detail = result.restore.unwrap();
            assert_eq!(result.diagnostic(), "IRIS-I106");
            assert!(result.manual);
            assert_eq!(detail.stage, stage);
            assert_eq!(detail.evidence.current, guard::Reason::ForegroundChanged);
            assert_eq!(f.checks, check);
            assert_eq!(f.replies.len(), remaining);
            assert_eq!(f.events, ["off", "send", "restore_wait"]);
            assert!(detail.display().contains("enqueued=6/6"));
            assert!(detail.display().contains("app_send_result=unknown"));
        }
    }

    /// オフ以外・取得不能では後段ガードを呼ばず、応答を成功と解釈しない。
    #[test]
    fn restore_read_detail_skips_unevaluated_guard() {
        for (reply, stage, text) in [
            (ImeReply::Raw(1), RestoreStage::ReadNonzero, "raw:1"),
            (ImeReply::Raw(2), RestoreStage::ReadNonzero, "raw:2"),
            (
                ImeReply::Failed(1460),
                RestoreStage::ReadUnknown,
                "query_failed:1460",
            ),
            (
                ImeReply::NotQueried,
                RestoreStage::ReadUnknown,
                "not_queried",
            ),
        ] {
            let mut f = Fake::on();
            f.replies[3] = reply;
            f.fail_check = 6;
            let result = run(&mut f);
            let detail = result.restore.unwrap();
            assert_eq!(detail.stage, stage);
            assert_eq!(f.checks, 5);
            assert_eq!(f.replies.len(), 2);
            assert_eq!(f.events, ["off", "send", "restore_wait"]);
            assert!(detail.display().contains(text));
            assert!(detail.display().contains("guard=not_evaluated"));
        }
    }

    /// 待機中の理由は後の中止フラグに上書きされず、未投入も区別する。
    #[test]
    fn restore_detail_keeps_wait_evidence_and_skipped_input() {
        let mut f = Fake::on();
        f.interrupt_on_wait = true;
        f.evidence = guard::Evidence {
            current: guard::Reason::Abort,
            wait: Some(guard::Reason::Deadline),
            ..guard::Evidence::UNKNOWN
        };
        let result = run(&mut f);
        let detail = result.restore.unwrap();
        assert_eq!(detail.stage, RestoreStage::BeforeGuard);
        assert!(detail
            .display()
            .contains("guard=abort_flag wait_guard=deadline"));
        assert_eq!(f.events, ["off", "send", "restore_wait"]);

        let mut f = Fake::on();
        f.input.attempted = false;
        f.input.inserted = 0;
        f.fail_check = 5;
        let result = run(&mut f);
        assert!(result
            .restore
            .unwrap()
            .display()
            .contains("input=not_attempted"));
        assert_eq!(f.events, ["off", "send"]);
    }

    /// 他の診断コードや正常な復元に I106 の詳細を付けない。
    #[test]
    fn restore_detail_is_scoped_to_i106() {
        for index in [0, 1, 2, 4, 5] {
            let mut f = Fake::on();
            f.replies[index] = ImeReply::Failed(0);
            assert!(run(&mut f).restore.is_none());
        }
        let mut f = Fake::on();
        assert!(run(&mut f).restore.is_none());
        let mut f = Fake::off();
        assert!(run(&mut f).restore.is_none());
    }
}
