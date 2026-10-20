pub type Result<T> = std::result::Result<T, &'static str>;

/// 保存したWin32/IME操作先が全項目既知かつ一致するときだけ復元を許可する。
pub fn check_identity(values: [Option<bool>; 5]) -> Result<()> {
    for (value, reason) in values.into_iter().zip([
        "restore_foreground_changed",
        "restore_win32_focus_changed",
        "restore_ime_window_changed",
        "restore_ime_owner_changed",
        "restore_layout_changed",
    ]) {
        match value {
            Some(true) => {}
            Some(false) => return Err(reason),
            None => return Err("restore_identity_unknown"),
        }
    }
    Ok(())
}

pub trait Probe {
    type Focus;
    /// 元要求の固定履歴・前面・期限を確認する。
    fn check(&mut self) -> Result<()>;
    /// 保存ルートの同一性と公開阻害要素を確認する。
    fn root(&mut self) -> Result<()>;
    /// 保存したWin32入力先・IME操作先・layoutとの一致を確認する。
    fn identity(&mut self) -> Result<()>;
    /// 現在フォーカスを取得する。送信候補の選択ではない。
    fn focus(&mut self) -> Result<Self::Focus>;
    /// 現在フォーカスが保存ルート配下か確認する。
    fn ancestry(&mut self, focus: &Self::Focus) -> Result<()>;
    /// 復元検証の途中で現在フォーカスが変わっていないか確認する。
    fn same_focus(&mut self, focus: &Self::Focus) -> Result<bool>;
}

/// 旧UIA要素に依存せず、保存IME操作先の後処理として復元を検証する。
pub fn validate<P: Probe>(probe: &mut P) -> Result<()> {
    probe.check()?;
    probe.root()?;
    probe.identity()?;
    probe.check()?;
    let focus = probe.focus()?;
    probe.check()?;
    probe.ancestry(&focus)?;
    probe.check()?;
    probe.root()?;
    if !probe.same_focus(&focus)? {
        return Err("restore_focus_changed_during_check");
    }
    probe.identity()?;
    probe.check()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[derive(Clone, Copy, Debug)]
    pub enum Case {
        Unchanged,
        ReplacedUia,
        RootChanged,
        AncestryUnknown,
        FocusReadFailed,
        CompareFailed,
        FocusChanged,
        ForegroundChanged,
        Win32FocusChanged,
        ImeWindowChanged,
        ImeOwnerChanged,
        LayoutChanged,
        IdentityUnknown,
        Intervention,
        Deadline,
    }
    pub const CASES: [Case; 15] = [
        Case::Unchanged,
        Case::ReplacedUia,
        Case::RootChanged,
        Case::AncestryUnknown,
        Case::FocusReadFailed,
        Case::CompareFailed,
        Case::FocusChanged,
        Case::ForegroundChanged,
        Case::Win32FocusChanged,
        Case::ImeWindowChanged,
        Case::ImeOwnerChanged,
        Case::LayoutChanged,
        Case::IdentityUnknown,
        Case::Intervention,
        Case::Deadline,
    ];

    struct Fake {
        case: Case,
        checks: usize,
        stop_at: Option<usize>,
        roots: usize,
        root_change_at: Option<usize>,
        identities: usize,
        identity_change_at: Option<usize>,
    }
    impl Fake {
        /// 元のUIA要素IDは1、再生成後は2として観測を作る。
        fn new(case: Case) -> Self {
            Self {
                case,
                checks: 0,
                stop_at: None,
                roots: 0,
                root_change_at: None,
                identities: 0,
                identity_change_at: None,
            }
        }
    }
    impl Probe for Fake {
        type Focus = u32;
        /// 各境界の介入と期限を再現する。
        fn check(&mut self) -> Result<()> {
            self.checks += 1;
            if self.stop_at == Some(self.checks) || matches!(self.case, Case::Intervention) {
                Err("intervention")
            } else if matches!(self.case, Case::Deadline) {
                Err("deadline")
            } else {
                Ok(())
            }
        }
        /// 後半でのルート差替えも検出する。
        fn root(&mut self) -> Result<()> {
            self.roots += 1;
            if matches!(self.case, Case::RootChanged) || self.root_change_at == Some(self.roots) {
                Err("root_changed")
            } else {
                Ok(())
            }
        }
        /// 保存操作先の不一致と取得不能を区別する。
        fn identity(&mut self) -> Result<()> {
            self.identities += 1;
            let mut values = [Some(true); 5];
            let index = match self.case {
                Case::ForegroundChanged => Some(0),
                Case::Win32FocusChanged => Some(1),
                Case::ImeWindowChanged => Some(2),
                Case::ImeOwnerChanged => Some(3),
                Case::LayoutChanged => Some(4),
                _ => None,
            };
            if let Some(index) = index {
                values[index] = Some(false);
            }
            if matches!(self.case, Case::IdentityUnknown) {
                values[2] = None;
            }
            if self.identity_change_at == Some(self.identities) {
                values[1] = Some(false);
            }
            check_identity(values)
        }
        /// 新規取得のフォーカスは旧要素と異なっていてもよい。
        fn focus(&mut self) -> Result<u32> {
            if matches!(self.case, Case::FocusReadFailed) {
                Err("uia_error")
            } else {
                Ok(if matches!(self.case, Case::ReplacedUia) {
                    2
                } else {
                    1
                })
            }
        }
        /// 現在フォーカスのルート所属不明は拒否する。
        fn ancestry(&mut self, _: &u32) -> Result<()> {
            if matches!(self.case, Case::AncestryUnknown) {
                Err("ancestry_unknown")
            } else {
                Ok(())
            }
        }
        /// 旧要素ではなく、この検証中に取得した要素との一致を返す。
        fn same_focus(&mut self, focus: &u32) -> Result<bool> {
            if matches!(self.case, Case::CompareFailed) {
                Err("uia_error")
            } else {
                Ok(!matches!(self.case, Case::FocusChanged)
                    && *focus
                        == if matches!(self.case, Case::ReplacedUia) {
                            2
                        } else {
                            1
                        })
            }
        }
    }

    /// 復元だけは再生成を許容し、送信前の同一性検証は拒否を維持する。
    #[test]
    fn replaced_uia_is_allowed_only_for_restoration() {
        assert!(validate_case(Case::ReplacedUia));
        let mut send = crate::selection::tests::Fake::new(&[true]);
        send.items[0].0 = 2;
        assert_eq!(
            crate::selection::revalidate(&mut send, false, &1),
            Err("target_changed")
        );
        for changed in [1, 2, 3, 4, 5, 6, 7] {
            assert!(!crate::selection::tests::revalidate_case(changed));
        }
    }

    /// 別操作先・未知・介入は同じHWNDに見えても許可しない。
    #[test]
    fn all_mismatches_and_failures_stop_restore() {
        for case in CASES {
            assert_eq!(
                validate_case(case),
                matches!(case, Case::Unchanged | Case::ReplacedUia),
                "{case:?}"
            );
        }
        for index in 0..5 {
            let mut values = [Some(true); 5];
            values[index] = None;
            assert_eq!(check_identity(values), Err("restore_identity_unknown"));
        }
        let mut p = Fake::new(Case::Unchanged);
        p.root_change_at = Some(2);
        assert_eq!(validate(&mut p), Err("root_changed"));
        let mut p = Fake::new(Case::Unchanged);
        p.identity_change_at = Some(2);
        assert_eq!(validate(&mut p), Err("restore_win32_focus_changed"));
    }

    /// 全ガード区間での介入をラッチ解除や再試行で救済しない。
    #[test]
    fn intervention_at_every_boundary_stops_restore() {
        let mut baseline = Fake::new(Case::Unchanged);
        assert_eq!(validate(&mut baseline), Ok(()));
        for at in 1..=baseline.checks {
            let mut p = Fake::new(Case::Unchanged);
            p.stop_at = Some(at);
            assert_eq!(validate(&mut p), Err("intervention"));
        }
    }

    /// WriterとIMEモデルも本番と同じ復元判定を使って試験する。
    pub fn validate_case(case: Case) -> bool {
        validate(&mut Fake::new(case)).is_ok()
    }
}
