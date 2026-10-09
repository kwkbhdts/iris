use super::diagnostic::ImeReply;

#[cfg(windows)]
pub const HOLD_MS: u32 = 100;
pub const DEADLINE_MS: u32 = 500;

/// 時計の周回を考慮し、古い試験を再開させない。
pub fn within_deadline(start: u32, now: u32) -> bool {
    now.wrapping_sub(start) <= DEADLINE_MS
}

pub trait Backend {
    /// 対象・介入・期限を確認する。終了要求だけなら復元の余地を残す。
    fn guarded(&mut self) -> bool;
    /// 終了要求を確認する。
    fn closing(&self) -> bool;
    /// 従来の応答だけを取得する。
    fn read_open(&mut self) -> ImeReply;
    /// 開閉の要求結果を取得する。配送成功だけで状態変更を断定しない。
    fn set_open(&mut self, open: bool) -> ImeReply;
    /// 短い待機。終了・介入・期限で早期に戻る。
    fn hold(&mut self);
}

pub struct Outcome {
    pub status: &'static str,
    pub manual: bool,
    pub trace: Vec<(&'static str, ImeReply)>,
}

impl Outcome {
    /// 生の応答と手動確認の要否だけを表示する。
    pub fn render(&self) -> String {
        let mut text = format!(
            "trial_status={}\nmanual_check_required={}\n",
            self.status, self.manual
        );
        for (name, value) in &self.trace {
            text.push_str(&format!("{name}: {}\n", value.describe()));
        }
        if self.manual {
            text.push_str("手動確認が必要です。元の入力欄で IME を確認・復元してください。再試験には終了後の再起動が必要です。\n");
        }
        text
    }
}

/// 一件の試験を順に実行する。書き込み後の不明・介入では盲目的に復元しない。
pub fn run(backend: &mut impl Backend) -> Outcome {
    let mut out = Outcome {
        status: "skipped_before_change",
        manual: false,
        trace: Vec::new(),
    };
    if backend.closing() || !backend.guarded() {
        return out;
    }
    let before = backend.read_open();
    out.trace.push(("open_before", before));
    if !matches!(before, ImeReply::Raw(1)) {
        out.status = "original_not_one; unchanged";
        return out;
    }
    if backend.closing() || !backend.guarded() {
        return out;
    }

    // 呼出し開始後は失敗でも変更された可能性がある。完全な復元まで不明扱い。
    out.manual = true;
    out.status = "off_request_unconfirmed";
    let off = backend.set_open(false);
    out.trace.push(("set_off_reply", off));
    if !matches!(off, ImeReply::Raw(0)) || !backend.guarded() {
        return out;
    }
    let after_off = backend.read_open();
    out.trace.push(("open_after_off", after_off));
    if !matches!(after_off, ImeReply::Raw(0)) || !backend.guarded() {
        return out;
    }

    backend.hold();
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
    let restore = backend.set_open(true);
    out.trace.push(("set_on_reply", restore));
    if !matches!(restore, ImeReply::Raw(0)) || !backend.guarded() {
        return out;
    }
    let restored = backend.read_open();
    out.trace.push(("open_after_restore", restored));
    if matches!(restored, ImeReply::Raw(1)) && backend.guarded() {
        out.manual = false;
        out.status = "legacy_1_0_1_observed; actual_tsf_state_unverified";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Fake {
        replies: VecDeque<ImeReply>,
        writes: Vec<bool>,
        valid: bool,
        closing: bool,
        interrupt_on_hold: bool,
        close_on_hold: bool,
        checks: usize,
        fail_check: usize,
    }
    impl Fake {
        /// 正常な往復の応答列を用意する。
        fn new() -> Self {
            Self {
                replies: [1, 0, 0, 0, 0, 1].map(ImeReply::Raw).into(),
                writes: vec![],
                valid: true,
                closing: false,
                interrupt_on_hold: false,
                close_on_hold: false,
                checks: 0,
                fail_check: usize::MAX,
            }
        }
    }
    impl Backend for Fake {
        fn guarded(&mut self) -> bool {
            self.checks += 1;
            self.valid && self.checks < self.fail_check
        }
        fn closing(&self) -> bool {
            self.closing
        }
        fn read_open(&mut self) -> ImeReply {
            self.replies.pop_front().unwrap()
        }
        fn set_open(&mut self, open: bool) -> ImeReply {
            self.writes.push(open);
            self.read_open()
        }
        fn hold(&mut self) {
            self.valid &= !self.interrupt_on_hold;
            self.closing |= self.close_on_hold;
        }
    }

    /// 期限の境界と時計の周回を検証する。
    #[test]
    fn deadline_includes_boundary_and_handles_wrap() {
        let start = u32::MAX - 20;
        assert!(within_deadline(start, start.wrapping_add(500)));
        assert!(!within_deadline(start, start.wrapping_add(501)));
    }

    /// 一回のオフとオンだけで往復し、TSF 成功とは表示しない。
    #[test]
    fn round_trip_writes_once_each() {
        let mut f = Fake::new();
        let result = run(&mut f);
        assert_eq!(f.writes, [false, true]);
        assert!(!result.manual);
        assert!(result.render().contains("actual_tsf_state_unverified"));
    }

    /// 初期値が正確に1でなければ書き込まない。
    #[test]
    fn zero_unknown_and_failure_do_not_write() {
        for value in [
            ImeReply::Raw(0),
            ImeReply::Raw(2),
            ImeReply::NotQueried,
            ImeReply::Failed(0),
        ] {
            let mut f = Fake::new();
            f.replies[0] = value;
            assert!(!run(&mut f).manual);
            assert!(f.writes.is_empty());
        }
    }

    /// 開始前の終了要求は書き込みを止める。
    #[test]
    fn close_before_off_is_unchanged() {
        let mut f = Fake::new();
        f.closing = true;
        assert!(!run(&mut f).manual);
        assert!(f.writes.is_empty());
    }

    /// 途中の終了要求でも対象が保持されていれば一回だけ戻す。
    #[test]
    fn close_after_off_restores_under_the_same_guard() {
        let mut f = Fake::new();
        f.close_on_hold = true;
        assert!(!run(&mut f).manual);
        assert_eq!(f.writes, [false, true]);
    }

    /// 介入・対象変更・期限切れ後には戻さない。
    #[test]
    fn invalidated_hold_requires_manual_recovery() {
        let mut f = Fake::new();
        f.interrupt_on_hold = true;
        assert!(run(&mut f).manual);
        assert_eq!(f.writes, [false]);
    }

    /// オフ要求のタイムアウトは未変更とみなさない。
    #[test]
    fn timed_out_write_is_uncertain_without_retry() {
        let mut f = Fake::new();
        f.replies[1] = ImeReply::Failed(1460);
        assert!(run(&mut f).manual);
        assert_eq!(f.writes, [false]);
    }

    /// 復元直前が不明・既にオンなら設定で上書きしない。
    #[test]
    fn uncertain_or_changed_state_prevents_restore() {
        for value in [ImeReply::Failed(0), ImeReply::Raw(1), ImeReply::Raw(2)] {
            let mut f = Fake::new();
            f.replies[3] = value;
            assert!(run(&mut f).manual);
            assert_eq!(f.writes, [false]);
        }
    }

    /// 復元応答を取得できなければ成功と表示しない。
    #[test]
    fn restore_failure_requires_manual_check() {
        for index in [4, 5] {
            let mut f = Fake::new();
            f.replies[index] = ImeReply::Failed(0);
            assert!(run(&mut f).manual);
            assert_eq!(f.writes, [false, true]);
        }
    }

    /// 各再確認点で失効しても、それ以降の書き込みを実行しない。
    #[test]
    fn each_guard_can_stop_the_transaction() {
        for stop in 1..=8 {
            let mut f = Fake::new();
            f.fail_check = stop;
            let result = run(&mut f);
            assert!(f.writes.len() <= 2);
            if stop <= 2 {
                assert!(f.writes.is_empty());
                assert!(!result.manual);
            } else {
                assert!(result.manual);
            }
            if stop <= 6 {
                assert!(!f.writes.contains(&true));
            }
        }
    }
}
