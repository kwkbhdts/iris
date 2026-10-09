use super::diagnostic::ImeReply;

pub const RESTORE_DELAY_MS: u32 = 100;
pub const DEADLINE_MS: u32 = 500;
pub const INPUT_COUNT: u32 = 6;

/// 時計の周回を扱い、期限切れの入力・復元を再開させない。
pub fn within_deadline(start: u32, now: u32) -> bool {
    now.wrapping_sub(start) <= DEADLINE_MS
}

/// 自分で注入したイベントだけを介入判定から除く。本人確認には使用しない。
pub fn own_input(injected: bool, tag: usize, expected: usize) -> bool {
    injected && tag == expected
}

#[derive(Clone, Copy)]
pub struct InputReply {
    pub attempted: bool,
    pub inserted: u32,
    pub error: u32,
}

pub trait Backend {
    /// 対象・介入・期限・レイアウトを再確認する。
    fn guarded(&mut self) -> bool;
    /// 終了要求後には新たな入力を送らない。
    fn closing(&self) -> bool;
    /// 従来の IME 開閉応答を取得する。
    fn read_open(&mut self) -> ImeReply;
    /// 開閉だけを設定する。
    fn set_open(&mut self, open: bool) -> ImeReply;
    /// 最後にガードを確認し、待機を挟まず OK と Enter を一度だけ投入する。
    fn send_once(&mut self) -> InputReply;
    /// 入力後の復元だけの暫定待機。アプリの処理完了確認ではない。
    fn wait_before_restore(&mut self);
}

pub struct Outcome {
    pub status: &'static str,
    pub manual: bool,
    pub input: Option<InputReply>,
    pub trace: Vec<(&'static str, ImeReply)>,
}

impl Outcome {
    /// 投入結果と IME 応答を表示し、実際の送信成功とは表現しない。
    pub fn render(&self) -> String {
        let mut text = format!("trial_status={}\nmanual_check_required={}\ntext_to_enter_wait_ms=0\nrestore_delay_if_original_on_ms={} (experimental; not_app_completion)\n", self.status, self.manual, RESTORE_DELAY_MS);
        if let Some(input) = self.input {
            text.push_str(&format!(
                "send_input_attempted={}; inserted={}/{}; error={}\n",
                input.attempted, input.inserted, INPUT_COUNT, input.error
            ));
        } else {
            text.push_str("send_input_attempted=false\n");
        }
        text.push_str("app_consumption=unknown; chat_send_success=unknown\n");
        for (name, value) in &self.trace {
            text.push_str(&format!("{name}: {}\n", value.describe()));
        }
        if self.manual {
            text.push_str("手動確認が必要です。入力・送信結果、キー状態、IME を確認してください。再試験は手動確認後に終了・再起動してください。\n");
        }
        text
    }
}

/// 元の応答0/1を分岐し、入力一回と条件付き復元だけを実行する。
pub fn run(backend: &mut impl Backend) -> Outcome {
    let mut out = Outcome {
        status: "skipped_before_change",
        manual: false,
        input: None,
        trace: vec![],
    };
    if backend.closing() || !backend.guarded() {
        return out;
    }
    let original = backend.read_open();
    out.trace.push(("open_before", original));
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
        out.trace.push(("set_off_reply", off));
        if !matches!(off, ImeReply::Raw(0)) || !backend.guarded() {
            return out;
        }
    }

    let off = backend.read_open();
    out.trace.push(("open_before_input", off));
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
    if !backend.guarded() {
        return out;
    }
    let current = backend.read_open();
    out.trace.push(("open_before_restore", current));
    if !matches!(current, ImeReply::Raw(0)) || !backend.guarded() {
        return out;
    }
    out.status = "restore_request_unconfirmed";
    let restored = backend.set_open(true);
    out.trace.push(("set_on_reply", restored));
    if !matches!(restored, ImeReply::Raw(0)) || !backend.guarded() {
        return out;
    }
    let after = backend.read_open();
    out.trace.push(("open_after_restore", after));
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
                    error: 0,
                },
                valid: true,
                close: false,
                close_after_off: false,
                interrupt_on_wait: false,
                fail_check: usize::MAX,
                checks: 0,
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
        assert!(r.render().contains("chat_send_success=unknown"));
        assert!(r.render().contains("text_to_enter_wait_ms=0"));
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
                f.input.error = 5;
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
            error: 0,
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
}
