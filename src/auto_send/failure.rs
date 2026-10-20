use crate::{ancestry, types::WindowId};
pub type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, Default)]
pub struct Identity {
    pub foreground: Option<WindowId>,
    pub focus: Option<WindowId>,
    pub ime: Option<WindowId>,
    pub layout: Option<usize>,
}

/// 取得できない情報を不一致へ読み替えない。
fn equal<T: PartialEq>(before: Option<T>, after: Option<T>) -> Option<bool> {
    before.zip(after).map(|(a, b)| a == b)
}

/// 生の識別情報を保持したまま出力せず、一致真偽だけを作る。
pub fn compare(before: Identity, after: Identity) -> [Option<bool>; 5] {
    [
        equal(before.foreground, after.foreground),
        equal(before.focus, after.focus),
        equal(before.ime, after.ime),
        equal(
            before.ime.map(|w| (w.pid, w.tid)),
            after.ime.map(|w| (w.pid, w.tid)),
        ),
        equal(before.layout, after.layout),
    ]
}

pub trait Probe {
    type Node;
    /// 失敗ラッチを解除せず、元要求の前面・介入・期限を確認する。
    fn guard(&mut self) -> Result<()>;
    /// 保存Win32入力先との一致だけを観測する。
    fn identity(&mut self) -> [Option<bool>; 5];
    /// 現在のUIA実フォーカスを新たに取得する。
    fn focus(&mut self) -> Result<Self::Node>;
    /// 保存要素とのUIA同一性を読む。
    fn same(&mut self, node: &Self::Node) -> Result<bool>;
    /// 新しい実フォーカスから保存ルートへの到達を調べる。
    fn path(&mut self, node: Self::Node) -> ancestry::Observation;
}

pub struct Sample {
    pub identity: [Option<bool>; 5],
    pub same: Option<bool>,
    pub path: ancestry::Observation,
    pub complete: bool,
    pub reason: &'static str,
}

/// 失敗後の一回限りの読取りであり、許可判定や書込みの経路は持たない。
pub fn observe<P: Probe>(probe: &mut P) -> Sample {
    let mut sample = Sample {
        identity: [None; 5],
        same: None,
        path: ancestry::Observation::unknown("not_observed"),
        complete: false,
        reason: "none",
    };
    let result = (|| {
        probe.guard()?;
        sample.identity = probe.identity();
        probe.guard()?;
        let focus = probe.focus()?;
        probe.guard()?;
        match probe.same(&focus) {
            Ok(same) => sample.same = Some(same),
            Err(reason) => sample.reason = reason,
        }
        probe.guard()?;
        sample.path = probe.path(focus);
        probe.guard()?;
        Ok(())
    })();
    if let Err(reason) = result {
        sample.reason = reason;
    }
    sample.complete = result.is_ok()
        && sample.same.is_some()
        && sample.path.complete
        && sample.identity.iter().all(Option::is_some);
    if !sample.complete && sample.reason == "none" {
        sample.reason = if !sample.path.complete {
            sample.path.reason
        } else {
            "identity_unknown"
        };
    }
    sample
}

/// 三値の観測をログ用の固定語へ変換する。
pub fn label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    struct Fake {
        checks: usize,
        stop_at: Option<usize>,
        focus_error: bool,
        same: Result<bool>,
        path_ok: bool,
        reads: usize,
    }
    impl Probe for Fake {
        type Node = ();
        /// 指定位置の介入を模擬する。
        fn guard(&mut self) -> Result<()> {
            self.checks += 1;
            if self.stop_at == Some(self.checks) {
                Err("intervention")
            } else {
                Ok(())
            }
        }
        /// 既知の一致を返す。
        fn identity(&mut self) -> [Option<bool>; 5] {
            self.reads += 1;
            [Some(true); 5]
        }
        /// 取得失敗を別要素と混同しない。
        fn focus(&mut self) -> Result<()> {
            self.reads += 1;
            if self.focus_error {
                Err("uia_error")
            } else {
                Ok(())
            }
        }
        /// 一致・不一致・失敗を返す。
        fn same(&mut self, _: &()) -> Result<bool> {
            self.reads += 1;
            self.same
        }
        /// 祖先unknownを維持する。
        fn path(&mut self, _: ()) -> ancestry::Observation {
            self.reads += 1;
            if self.path_ok {
                ancestry::Observation {
                    root_reached: true,
                    complete: true,
                    document: Some(false),
                    reason: "none",
                    parent_missing_depth: None,
                }
            } else {
                ancestry::Observation::unknown("root_not_reached")
            }
        }
    }
    /// 一回の診断用プローブを用意する。
    fn fake() -> Fake {
        Fake {
            checks: 0,
            stop_at: None,
            focus_error: false,
            same: Ok(true),
            path_ok: true,
            reads: 0,
        }
    }

    /// 診断が全一致でも、呼出側の復元拒否を解除しない試験に使う。
    pub fn matching_sample() -> Sample {
        observe(&mut fake())
    }

    /// UIA取得失敗はfalseにせず、祖先不明も成功扱いしない。
    #[test]
    fn observations_distinguish_mismatch_from_unknown() {
        for same in [Ok(true), Ok(false), Err("uia_error")] {
            let mut p = fake();
            p.same = same;
            let sample = observe(&mut p);
            assert_eq!(sample.same, same.ok());
            assert_eq!(sample.complete, same.is_ok());
            assert_eq!(p.reads, 4);
        }
        let mut p = fake();
        p.focus_error = true;
        let sample = observe(&mut p);
        assert_eq!(sample.same, None);
        assert!(!sample.complete);
        assert_eq!(p.reads, 2);
        let mut p = fake();
        p.path_ok = false;
        let sample = observe(&mut p);
        assert!(!sample.complete);
        assert_eq!(sample.path.reason, "root_not_reached");
    }

    /// 各境界の介入では続行せず、再観測もしない。
    #[test]
    fn guards_stop_reads_without_retry() {
        let mut baseline = fake();
        assert!(observe(&mut baseline).complete);
        for at in 1..=baseline.checks {
            let mut p = fake();
            p.stop_at = Some(at);
            let sample = observe(&mut p);
            assert!(!sample.complete);
            assert_eq!(sample.reason, "intervention");
            assert_eq!(p.checks, at);
            if at == 1 {
                assert_eq!(p.reads, 0);
            }
        }
    }

    /// Win32入力先・所有者・layoutの未知と不一致を区別する。
    #[test]
    fn identity_comparison_is_three_valued() {
        let w = WindowId {
            hwnd: 1,
            pid: 2,
            tid: 3,
        };
        let before = Identity {
            foreground: Some(w),
            focus: Some(w),
            ime: Some(w),
            layout: Some(4),
        };
        assert_eq!(compare(before, before), [Some(true); 5]);
        assert_eq!(compare(before, Identity::default()), [None; 5]);
        let after = Identity {
            foreground: Some(WindowId { hwnd: 9, ..w }),
            focus: None,
            ime: Some(WindowId { pid: 9, ..w }),
            layout: None,
        };
        assert_eq!(
            compare(before, after),
            [Some(false), None, Some(false), Some(false), None]
        );
        assert_eq!(label(None), "unknown");
    }
}
