pub type Result<T> = std::result::Result<T, &'static str>;

/// 初期フォーカスの文書を受け取らず、候補探索をルート検証で挟む。
pub trait Probe {
    type Target;
    fn check_root(&mut self) -> Result<()>;
    fn find_target(&mut self) -> Result<Self::Target>;
}

/// 探索中にダイアログや前面変化が生じても、候補を返して進めない。
pub fn run<P: Probe>(probe: &mut P) -> Result<P::Target> {
    probe.check_root()?;
    let target = probe.find_target()?;
    probe.check_root()?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Fake {
        checks: VecDeque<Result<()>>,
        target: Result<u8>,
        calls: Vec<&'static str>,
    }
    impl Probe for Fake {
        type Target = u8;
        fn check_root(&mut self) -> Result<()> {
            self.calls.push("check");
            self.checks.pop_front().unwrap()
        }
        fn find_target(&mut self) -> Result<u8> {
            self.calls.push("search");
            self.target
        }
    }

    /// 初期文書がなくても、候補側の検証成功と前後のルート確認で採用する。
    #[test]
    fn root_selection_needs_no_initial_document() {
        let mut probe = Fake {
            checks: [Ok(()), Ok(())].into(),
            target: Ok(7),
            calls: vec![],
        };
        assert_eq!(run(&mut probe), Ok(7));
        assert_eq!(probe.calls, ["check", "search", "check"]);
    }

    /// 背後に候補があっても、明示ダイアログや確認失敗を無視しない。
    #[test]
    fn blockers_and_failures_stop_before_search() {
        for reason in [
            "visible_dialog_or_modal",
            "root_changed",
            "uia_error",
            "deadline",
        ] {
            let mut probe = Fake {
                checks: [Err(reason)].into(),
                target: Ok(7),
                calls: vec![],
            };
            assert_eq!(run(&mut probe), Err(reason));
            assert_eq!(probe.calls, ["check"]);
        }
    }

    /// 一意に見つかった後のモーダル出現・確認失敗でも候補を返さない。
    #[test]
    fn change_during_search_discards_target() {
        for reason in [
            "visible_dialog_or_modal",
            "root_changed",
            "uia_error",
            "deadline",
        ] {
            let mut probe = Fake {
                checks: [Ok(()), Err(reason)].into(),
                target: Ok(7),
                calls: vec![],
            };
            assert_eq!(run(&mut probe), Err(reason));
            assert_eq!(probe.calls, ["check", "search", "check"]);
        }
    }

    /// 候補なし・複数・探索失敗を再試行や成功へ変えない。
    #[test]
    fn failed_selection_is_not_retried() {
        for reason in ["no_match", "ambiguous", "ancestor_limit", "uia_error"] {
            let mut probe = Fake {
                checks: [Ok(())].into(),
                target: Err(reason),
                calls: vec![],
            };
            assert_eq!(run(&mut probe), Err(reason));
            assert_eq!(probe.calls, ["check", "search"]);
        }
    }
}
