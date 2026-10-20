use crate::{ancestry, diagnostic::App};
pub type Result<T> = std::result::Result<T, &'static str>;

/// Document条件はブラウザーだけに適用する。
pub fn requires_document(app: App) -> bool {
    matches!(app, App::Chrome | App::Firefox)
}

pub trait Probe {
    type Element;
    /// 介入・期限を各区間で確認する。
    fn check(&mut self) -> Result<()>;
    /// 完了した5条件検索の全件数を返す。
    fn count(&mut self) -> Result<i32>;
    /// 候補を取得し、現在の5属性も再確認する。
    fn candidate(&mut self, index: i32) -> Result<Self::Element>;
    /// 全経路が確認できたときだけDocument祖先有無を返す。
    fn ancestry(&mut self, element: &Self::Element) -> Result<bool>;
    /// 再探索した唯一候補が元の対象か確認する。
    fn same(&mut self, before: &Self::Element, after: &Self::Element) -> Result<bool>;
    /// 元の件数と条件追加後の件数を分けて報告する。
    fn completed(&mut self, total: i32, eligible: usize);
}

/// 全候補を確認し終えるまで、途中の唯一候補を返さない。
pub fn select<P: Probe>(probe: &mut P, document_required: bool) -> Result<P::Element> {
    probe.check()?;
    let count = probe.count()?;
    if !(0..=ancestry::MAX_CANDIDATES).contains(&count) {
        return Err("candidate_limit");
    }
    let mut selected = None;
    let mut eligible = 0;
    for index in 0..count {
        probe.check()?;
        let element = probe.candidate(index)?;
        let document = probe.ancestry(&element)?;
        probe.check()?;
        if !document_required || document {
            eligible += 1;
            if selected.is_none() {
                selected = Some(element);
            }
        }
    }
    probe.check()?;
    probe.completed(count, eligible);
    match eligible {
        0 => Err("no_match"),
        1 => Ok(selected.expect("one eligible candidate")),
        _ => Err("ambiguous"),
    }
}

/// 同じ条件で再探索し、別の唯一候補へ切り替えない。
pub fn revalidate<P: Probe>(
    probe: &mut P,
    document_required: bool,
    expected: &P::Element,
) -> Result<P::Element> {
    let current = select(probe, document_required)?;
    if !probe.same(expected, &current)? {
        return Err("target_changed");
    }
    probe.check()?;
    Ok(current)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct Fake {
        pub items: Vec<(u32, bool)>,
        pub count_override: Option<Result<i32>>,
        pub error_at: Option<(usize, &'static str)>,
        pub ancestor_error: Option<(u32, &'static str)>,
        pub checks: usize,
        pub interrupt_at: Option<usize>,
        pub visited: Vec<u32>,
        pub summaries: Vec<(i32, usize)>,
        pub compare_error: bool,
    }
    impl Fake {
        /// 返却順とは別の要素IDを持つ模擬UIA検索を用意する。
        pub fn new(documents: &[bool]) -> Self {
            Self {
                items: documents
                    .iter()
                    .enumerate()
                    .map(|(i, d)| (i as u32 + 1, *d))
                    .collect(),
                count_override: None,
                error_at: None,
                ancestor_error: None,
                checks: 0,
                interrupt_at: None,
                visited: vec![],
                summaries: vec![],
                compare_error: false,
            }
        }
    }
    impl Probe for Fake {
        type Element = u32;
        /// 指定区間の介入を再現する。
        fn check(&mut self) -> Result<()> {
            self.checks += 1;
            if self.interrupt_at == Some(self.checks) {
                Err("intervention")
            } else {
                Ok(())
            }
        }
        /// 件数照会の失敗と不正値も再現する。
        fn count(&mut self) -> Result<i32> {
            self.count_override.unwrap_or(Ok(self.items.len() as i32))
        }
        /// 消失・属性変化を要素取得時の失敗として再現する。
        fn candidate(&mut self, index: i32) -> Result<u32> {
            if let Some((at, reason)) = self.error_at {
                if at == index as usize {
                    return Err(reason);
                }
            }
            Ok(self.items[index as usize].0)
        }
        /// 祖先unknownは結果全体の失敗にする。
        fn ancestry(&mut self, element: &u32) -> Result<bool> {
            self.visited.push(*element);
            if let Some((id, reason)) = self.ancestor_error {
                if id == *element {
                    return Err(reason);
                }
            }
            Ok(self.items.iter().find(|v| v.0 == *element).unwrap().1)
        }
        /// 要素同一性比較の失敗も再現する。
        fn same(&mut self, before: &u32, after: &u32) -> Result<bool> {
            if self.compare_error {
                Err("uia_error")
            } else {
                Ok(before == after)
            }
        }
        /// 全探索完了時だけ件数が報告されるか記録する。
        fn completed(&mut self, total: i32, eligible: usize) {
            self.summaries.push((total, eligible));
        }
    }

    /// ブラウザーだけDocument条件を使い、デスクトップは基本条件を保つ。
    #[test]
    fn app_policy_and_zero_one_multiple() {
        for app in [App::Chrome, App::Firefox, App::ChatGpt, App::Claude] {
            let doc = requires_document(app);
            assert_eq!(doc, matches!(app, App::Chrome | App::Firefox));
            assert_eq!(select(&mut Fake::new(&[]), doc), Err("no_match"));
            assert_eq!(select(&mut Fake::new(&[true]), doc), Ok(1));
            assert_eq!(
                select(&mut Fake::new(&[false]), doc),
                if doc { Err("no_match") } else { Ok(1) }
            );
            assert_eq!(select(&mut Fake::new(&[true, true]), doc), Err("ambiguous"));
            let mut p = Fake::new(&[false, true]);
            assert_eq!(
                select(&mut p, doc),
                if doc { Ok(2) } else { Err("ambiguous") }
            );
            assert_eq!(p.summaries, [(2, if doc { 1 } else { 2 })]);
            assert_eq!(p.visited, [1, 2]);
        }
    }

    /// 唯一候補の後も走査し、後続のunknownや属性変化を黙って除外しない。
    #[test]
    fn later_failure_never_promotes_a_partial_match() {
        for doc in [false, true] {
            for reason in [
                "root_not_reached",
                "ancestry_limit",
                "uia_error",
                "intervention",
                "deadline_exceeded",
            ] {
                let mut p = Fake::new(&[true, false]);
                p.ancestor_error = Some((2, reason));
                assert_eq!(select(&mut p, doc), Err(reason));
                assert!(p.summaries.is_empty());
            }
            for reason in ["element_disappeared", "candidate_attributes_changed"] {
                let mut p = Fake::new(&[true, false]);
                p.error_at = Some((1, reason));
                assert_eq!(select(&mut p, doc), Err(reason));
                assert!(p.summaries.is_empty());
            }
        }
    }

    /// 不正件数と上限超過は先頭候補を取得せず停止する。
    #[test]
    fn count_errors_and_limits_stop_without_truncation() {
        for count in [-1, ancestry::MAX_CANDIDATES + 1, i32::MAX] {
            let mut p = Fake::new(&[true]);
            p.count_override = Some(Ok(count));
            assert_eq!(select(&mut p, true), Err("candidate_limit"));
            assert!(p.visited.is_empty());
        }
        let mut p = Fake::new(&vec![false; ancestry::MAX_CANDIDATES as usize]);
        p.items.last_mut().unwrap().1 = true;
        assert_eq!(select(&mut p, true), Ok(ancestry::MAX_CANDIDATES as u32));
        let mut p = Fake::new(&[]);
        p.count_override = Some(Err("uia_error"));
        assert_eq!(select(&mut p, true), Err("uia_error"));
    }

    /// 再探索の全ガード位置で介入すると、比較後も結果を返さない。
    #[test]
    fn interruption_at_each_revalidation_boundary_stops() {
        let mut baseline = Fake::new(&[false, true]);
        assert_eq!(revalidate(&mut baseline, true, &2), Ok(2));
        for at in 1..=baseline.checks {
            let mut p = Fake::new(&[false, true]);
            p.interrupt_at = Some(at);
            assert_eq!(revalidate(&mut p, true, &2), Err("intervention"));
        }
    }

    /// 消失・別要素への差替え・増加・Document喪失・比較失敗を再探索で検出する。
    #[test]
    fn revalidation_never_switches_to_another_unique_element() {
        let expected = select(&mut Fake::new(&[false, true]), true).unwrap();
        assert_eq!(
            revalidate(&mut Fake::new(&[]), true, &expected),
            Err("no_match")
        );
        assert_eq!(
            revalidate(&mut Fake::new(&[true]), true, &expected),
            Err("target_changed")
        );
        assert_eq!(
            revalidate(&mut Fake::new(&[true, true]), true, &expected),
            Err("ambiguous")
        );
        assert_eq!(
            revalidate(&mut Fake::new(&[false, false]), true, &expected),
            Err("no_match")
        );
        let mut p = Fake::new(&[false, true]);
        p.compare_error = true;
        assert_eq!(revalidate(&mut p, true, &expected), Err("uia_error"));
        let mut p = Fake::new(&[false, true]);
        p.items.reverse();
        assert_eq!(revalidate(&mut p, true, &expected), Ok(expected));
    }

    /// Writerの試験でも本番と同じ再探索・同一性検証を通す。
    pub fn revalidate_case(changed: usize) -> bool {
        let mut p = Fake::new(if changed == 4 { &[true, true] } else { &[true] });
        if changed == 1 {
            p.items[0].0 = 99;
        }
        if !crate::policy::editable(true, changed != 3, true, changed == 2, false) {
            p.error_at = Some((0, "candidate_attributes_changed"));
        }
        if changed == 6 {
            p.ancestor_error = Some((1, "uia_error"));
        }
        revalidate(&mut p, true, &1).is_ok() && changed != 5
    }
}
