pub type Result<T> = std::result::Result<T, &'static str>;

/// 復元対象の属性。新規送信用のEnabled・Focusableは含めない。
#[derive(Clone, Copy)]
pub struct Attributes {
    pub edit: bool,
    pub password: bool,
    pub offscreen: bool,
}

pub trait Probe {
    /// 固定履歴・前面・期限を確認する。
    fn check(&mut self) -> Result<()>;
    /// 保存したルートの同一性と公開阻害要素を確認する。
    fn root(&mut self) -> Result<()>;
    /// 元要素のルート到達と、必要なDocument所属を確認する。
    fn ancestry(&mut self) -> Result<()>;
    /// 保存した元要素の属性だけを読む。
    fn attributes(&mut self) -> Result<Attributes>;
    /// 実フォーカスを元要素と比較する。別候補へ切り替えない。
    fn focused(&mut self) -> Result<bool>;
}

/// 元要素の入力先同一性を確認し、候補の再選択は行わない。
pub fn validate<P: Probe>(probe: &mut P) -> Result<()> {
    probe.check()?;
    probe.root()?;
    probe.check()?;
    probe.ancestry()?;
    probe.check()?;
    let attrs = probe.attributes()?;
    if !attrs.edit || attrs.password || attrs.offscreen {
        return Err("restore_target_attributes_changed");
    }
    probe.check()?;
    if !probe.focused()? {
        return Err("restore_focus_changed");
    }
    probe.check()?;
    probe.root()?;
    if !probe.focused()? {
        return Err("restore_focus_changed");
    }
    probe.check()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub struct Fake {
        pub attrs: Attributes,
        pub root_error: bool,
        pub ancestor_error: bool,
        pub read_error: bool,
        pub focus: bool,
        pub checks: usize,
        pub interrupt_at: Option<usize>,
        pub roots: usize,
        pub change_root_at: Option<usize>,
        pub focuses: usize,
        pub change_focus_at: Option<usize>,
    }
    impl Fake {
        /// 元の入力先が維持された応答を用意する。
        pub fn new() -> Self {
            Self {
                attrs: Attributes {
                    edit: true,
                    password: false,
                    offscreen: false,
                },
                root_error: false,
                ancestor_error: false,
                read_error: false,
                focus: true,
                checks: 0,
                interrupt_at: None,
                roots: 0,
                change_root_at: None,
                focuses: 0,
                change_focus_at: None,
            }
        }
    }
    impl Probe for Fake {
        /// 任意の区間での介入を再現する。
        fn check(&mut self) -> Result<()> {
            self.checks += 1;
            if self.interrupt_at == Some(self.checks) {
                Err("intervention")
            } else {
                Ok(())
            }
        }
        /// 最後の確認でルートが変わる場合も再現する。
        fn root(&mut self) -> Result<()> {
            self.roots += 1;
            if self.root_error || self.change_root_at == Some(self.roots) {
                Err("root_changed")
            } else {
                Ok(())
            }
        }
        /// 消失・所属不明は成功へ読み替えない。
        fn ancestry(&mut self) -> Result<()> {
            if self.ancestor_error {
                Err("ancestry_unknown")
            } else {
                Ok(())
            }
        }
        /// 属性取得失敗を再現する。
        fn attributes(&mut self) -> Result<Attributes> {
            if self.read_error {
                Err("uia_error")
            } else {
                Ok(self.attrs)
            }
        }
        /// 同一HWND内でも別のUIA要素への移動を拒否する。
        fn focused(&mut self) -> Result<bool> {
            self.focuses += 1;
            Ok(self.focus && self.change_focus_at != Some(self.focuses))
        }
    }

    /// 送信前に拒否する属性変化でも、元要素の復元条件が保たれれば許可する。
    #[test]
    fn enabled_focusable_and_candidate_count_are_send_only() {
        for (enabled, focusable) in [(false, true), (true, false), (false, false)] {
            assert!(!crate::policy::editable(
                true, enabled, focusable, false, false
            ));
            assert_eq!(validate(&mut Fake::new()), Ok(()));
        }
        for documents in [&[][..], &[true, true][..]] {
            let mut send = crate::selection::tests::Fake::new(documents);
            assert!(crate::selection::revalidate(&mut send, false, &1).is_err());
            assert_eq!(validate(&mut Fake::new()), Ok(()));
        }
    }

    /// 別要素・別ルート・取得失敗・非表示・型変更・passwordを拒否する。
    #[test]
    fn unknown_or_changed_input_target_stops_restore() {
        for case in [1, 2, 5, 6, 8, 9, 10, 11] {
            assert!(!validate_case(case));
        }
        let mut p = Fake::new();
        p.change_root_at = Some(2);
        assert_eq!(validate(&mut p), Err("root_changed"));
        let mut p = Fake::new();
        p.change_focus_at = Some(2);
        assert_eq!(validate(&mut p), Err("restore_focus_changed"));
    }

    /// 全ガード区間で介入すると復元検証を完了させない。
    #[test]
    fn intervention_at_every_boundary_stops_restore() {
        let mut baseline = Fake::new();
        assert_eq!(validate(&mut baseline), Ok(()));
        for at in 1..=baseline.checks {
            let mut p = Fake::new();
            p.interrupt_at = Some(at);
            assert_eq!(validate(&mut p), Err("intervention"));
        }
    }

    /// Writerの試験を本番と同じ復元検証へ接続する。
    pub fn validate_case(case: usize) -> bool {
        let mut p = Fake::new();
        match case {
            1 | 5 => p.focus = false,
            2 => p.attrs.offscreen = true,
            6 => p.ancestor_error = true,
            8 => p.root_error = true,
            9 => p.read_error = true,
            10 => p.attrs.password = true,
            11 => p.attrs.edit = false,
            _ => {} // 0=不変、3=Enabled変化、4=件数変化、7=Focusable変化。
        }
        validate(&mut p).is_ok()
    }
}
