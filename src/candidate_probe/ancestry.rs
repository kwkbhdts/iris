pub const MAX_CANDIDATES: i32 = 24;
pub const MAX_NODES: usize = 64;

type Result<T> = std::result::Result<T, &'static str>;

pub trait Tree {
    type Node;
    /// 要求の介入・前面・期限を確認する。
    fn check(&mut self) -> Result<()>;
    /// 要求内の前面ルートとの一致を調べる。
    fn is_root(&mut self, node: &Self::Node) -> Result<bool>;
    /// 祖先のDocument種別だけを読む。
    fn is_document(&mut self, node: &Self::Node) -> Result<bool>;
    /// 親なしと取得失敗を区別する。
    fn parent(&mut self, node: &Self::Node) -> Result<Option<Self::Node>>;
}

#[derive(Debug, PartialEq)]
pub struct Observation {
    pub root_reached: bool,
    pub complete: bool,
    pub document: Option<bool>,
    pub reason: &'static str,
    pub parent_missing_depth: Option<usize>,
}

impl Observation {
    /// 未到達・失敗をDocumentなしと混同しない。
    pub fn unknown(reason: &'static str) -> Self {
        Self {
            root_reached: false,
            complete: false,
            document: None,
            reason,
            parent_missing_depth: None,
        }
    }
}

/// 候補自身とルートをDocument判定から除き、上限内で祖先をたどる。
pub fn inspect<T: Tree>(tree: &mut T, candidate: T::Node) -> Observation {
    let mut node = candidate;
    let mut document = false;
    for depth in 0..MAX_NODES {
        let step = (|| {
            tree.check()?;
            if tree.is_root(&node)? {
                tree.check()?;
                return Ok(true);
            }
            if depth > 0 {
                document |= tree.is_document(&node)?;
            }
            tree.check()?;
            Ok(false)
        })();
        match step {
            Ok(true) => {
                return Observation {
                    root_reached: true,
                    complete: true,
                    document: Some(document),
                    reason: "none",
                    parent_missing_depth: None,
                }
            }
            Err(reason) => return Observation::unknown(reason),
            Ok(false) => {}
        }
        if depth + 1 == MAX_NODES {
            return Observation::unknown("ancestry_limit");
        }
        node = match tree.parent(&node) {
            Ok(Some(parent)) => parent,
            Ok(None) => {
                return Observation {
                    parent_missing_depth: Some(depth),
                    ..Observation::unknown("root_not_reached")
                }
            }
            Err(reason) => return Observation::unknown(reason),
        };
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        root: usize,
        document: Option<usize>,
        end: usize,
        fail: Option<usize>,
        getter_error: bool,
        checks: usize,
        interrupt: Option<usize>,
    }
    impl Tree for Fake {
        type Node = usize;
        fn check(&mut self) -> Result<()> {
            self.checks += 1;
            if self.interrupt == Some(self.checks) {
                Err("intervention")
            } else {
                Ok(())
            }
        }
        fn is_root(&mut self, node: &usize) -> Result<bool> {
            Ok(*node == self.root)
        }
        fn is_document(&mut self, node: &usize) -> Result<bool> {
            if self.getter_error {
                Err("uia_error")
            } else {
                Ok(Some(*node) == self.document)
            }
        }
        fn parent(&mut self, node: &usize) -> Result<Option<usize>> {
            if self.fail == Some(*node) {
                Err("uia_error")
            } else {
                Ok((*node < self.end).then_some(node + 1))
            }
        }
    }
    /// テスト用に候補・中間祖先・ルートの列を作る。
    fn tree() -> Fake {
        Fake {
            root: 3,
            document: Some(1),
            end: 3,
            fail: None,
            getter_error: false,
            checks: 0,
            interrupt: None,
        }
    }

    /// ルートへ届いた場合だけ有無を確定し、候補自身とルートを数えない。
    #[test]
    fn document_requires_ancestor_inside_root() {
        for (doc, expected) in [
            (None, false),
            (Some(0), false),
            (Some(1), true),
            (Some(3), false),
        ] {
            let mut t = tree();
            t.document = doc;
            assert_eq!(
                inspect(&mut t, 0),
                Observation {
                    root_reached: true,
                    complete: true,
                    document: Some(expected),
                    reason: "none",
                    parent_missing_depth: None
                }
            );
        }
    }

    /// 親なし・API失敗・深さ超過は、Documentを既に見てもunknownにする。
    #[test]
    fn unfinished_paths_are_unknown() {
        let mut t = tree();
        t.end = 2;
        assert_eq!(
            inspect(&mut t, 0),
            Observation {
                parent_missing_depth: Some(t.end),
                ..Observation::unknown("root_not_reached")
            }
        );
        let mut t = tree();
        t.end = 0;
        assert_eq!(
            inspect(&mut t, 0),
            Observation {
                parent_missing_depth: Some(t.end),
                ..Observation::unknown("root_not_reached")
            }
        );
        let mut t = tree();
        t.getter_error = true;
        assert_eq!(inspect(&mut t, 0), Observation::unknown("uia_error"));
        let mut t = tree();
        t.fail = Some(2);
        assert_eq!(inspect(&mut t, 0), Observation::unknown("uia_error"));
        let mut t = tree();
        t.root = MAX_NODES;
        t.end = MAX_NODES;
        assert_eq!(inspect(&mut t, 0), Observation::unknown("ancestry_limit"));
        t.root = MAX_NODES - 1;
        assert!(inspect(&mut t, 0).complete);
    }

    /// 各ガード位置の介入を未完了として扱う。
    #[test]
    fn intervention_at_every_check_is_unknown() {
        let mut baseline = tree();
        assert!(inspect(&mut baseline, 0).complete);
        for step in 1..=baseline.checks {
            let mut t = tree();
            t.interrupt = Some(step);
            assert_eq!(inspect(&mut t, 0), Observation::unknown("intervention"));
        }
    }
}
