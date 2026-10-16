#[derive(Default)]
pub struct Report {
    pub candidates: Option<i32>,
    pub blockers: Option<i32>,
    pub complete: bool,
}

impl Report {
    /// 途中観測や取得不能を、0件や唯一候補として報告しない。
    pub fn state(&self) -> &'static str {
        if !self.complete || !matches!(self.blockers, Some(0..)) {
            return "incomplete";
        }
        match self.candidates {
            Some(0) => "zero",
            Some(1) => "one",
            Some(2..) => "multiple",
            _ => "incomplete",
        }
    }

    /// 候補1件でも公開されたダイアログがあれば対象不可にする。
    pub fn eligible(&self) -> bool {
        self.state() == "one" && self.blockers == Some(0)
    }

    /// 観測前後のどちらかで見えた阻害要素を、後の0件で打ち消さない。
    pub fn observe_blockers(&mut self, count: i32) -> Result<(), &'static str> {
        if count < 0 {
            return Err("invalid_blocker_count");
        }
        self.blockers = Some(self.blockers.unwrap_or(0).max(count));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0・1・複数を区別し、アドレスバーと本文欄の2件も減らさない。
    #[test]
    fn exact_candidate_count_controls_eligibility() {
        for (count, state, eligible) in [
            (0, "zero", false),
            (1, "one", true),
            (2, "multiple", false),
            (1000, "multiple", false),
        ] {
            let report = Report {
                candidates: Some(count),
                blockers: Some(0),
                complete: true,
            };
            assert_eq!(report.state(), state);
            assert_eq!(report.eligible(), eligible);
        }
    }

    /// API失敗・ガード失敗後の途中件数を対象可へ昇格しない。
    #[test]
    fn partial_unknown_and_invalid_counts_are_incomplete() {
        for report in [
            Report {
                candidates: Some(1),
                blockers: Some(0),
                complete: false,
            },
            Report {
                candidates: None,
                blockers: Some(0),
                complete: true,
            },
            Report {
                candidates: Some(-1),
                blockers: Some(0),
                complete: true,
            },
            Report {
                candidates: Some(1),
                blockers: None,
                complete: true,
            },
            Report {
                candidates: Some(1),
                blockers: Some(-1),
                complete: true,
            },
        ] {
            assert_eq!(report.state(), "incomplete");
            assert!(!report.eligible());
        }
    }

    /// ダイアログの出現・消失どちらでも観測済みなら対象不可を保つ。
    #[test]
    fn any_observed_dialog_blocks_a_single_candidate() {
        for counts in [[0, 1], [1, 0]] {
            let mut report = Report {
                candidates: Some(1),
                complete: true,
                ..Report::default()
            };
            for count in counts {
                report.observe_blockers(count).unwrap();
            }
            assert_eq!(report.state(), "one");
            assert!(!report.eligible());
        }
        assert!(Report::default().observe_blockers(-1).is_err());
    }
}
