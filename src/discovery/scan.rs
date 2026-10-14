pub const NODE_LIMIT: usize = 2048;
pub const DEPTH_LIMIT: usize = 64;
pub const EDIT_LIMIT: usize = 24;
pub type Result<T> = std::result::Result<T, &'static str>;

/// プロバイダー呼び出しを有界探索から分離する。
pub trait Tree {
    type Node: Clone;
    /// 最初の子を返す。
    fn child(&mut self, node: &Self::Node) -> Result<Option<Self::Node>>;
    /// 次の兄弟を返す。
    fn sibling(&mut self, node: &Self::Node) -> Result<Option<Self::Node>>;
    /// ノードを観測し、子へ引き継ぐ文書所属を返す。
    fn visit(
        &mut self,
        node: &Self::Node,
        index: usize,
        parent: Option<usize>,
        depth: usize,
        in_document: bool,
    ) -> Result<bool>;
}

/// 全兄弟を辿るまで完了とせず、深さ・件数上限は途中終了にする。
pub fn walk<T: Tree>(
    tree: &mut T,
    root: T::Node,
    max_nodes: usize,
    max_depth: usize,
) -> Result<usize> {
    let mut current = root;
    let mut path = Vec::new();
    let mut parent = None;
    let mut in_document = false;
    let mut visited = 0;
    loop {
        if visited >= max_nodes {
            return Err("node_limit");
        }
        let index = visited;
        visited += 1;
        let document = tree.visit(&current, index, parent, path.len(), in_document)?;
        if let Some(child) = tree.child(&current)? {
            if path.len() + 1 >= max_depth {
                return Err("depth_limit");
            }
            path.push((current, parent, in_document));
            current = child;
            parent = Some(index);
            in_document = document;
            continue;
        }
        loop {
            if path.is_empty() {
                return Ok(visited);
            }
            if let Some(sibling) = tree.sibling(&current)? {
                current = sibling;
                break;
            }
            (current, parent, in_document) = path.pop().unwrap();
        }
    }
}

/// 比較用の一時値から真偽だけを返し、空同士や取得不能を同値扱いしない。
pub fn same_nonempty(a: Option<&[u16]>, b: Option<&[u16]>) -> Option<bool> {
    match (a, b) {
        (Some(a), Some(b)) => Some(!a.is_empty() && !b.is_empty() && a == b),
        _ => None,
    }
}

/// 診断上の一候補を、完了前や正解未検出時に一意と呼ばない。
pub fn unique(
    complete: bool,
    reference_is_edit: bool,
    candidates: usize,
    reference_matches: usize,
) -> bool {
    complete && reference_is_edit && candidates == 1 && reference_matches == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        fail_at: Option<usize>,
        visited: Vec<(usize, usize, Option<usize>, bool)>,
    }
    impl Tree for Fixture {
        type Node = usize;
        fn child(&mut self, node: &usize) -> Result<Option<usize>> {
            Ok(match node {
                0 => Some(1),
                1 => Some(2),
                _ => None,
            })
        }
        fn sibling(&mut self, node: &usize) -> Result<Option<usize>> {
            Ok(match node {
                1 => Some(4),
                2 => Some(3),
                _ => None,
            })
        }
        fn visit(
            &mut self,
            node: &usize,
            index: usize,
            parent: Option<usize>,
            _: usize,
            in_document: bool,
        ) -> Result<bool> {
            if self.fail_at == Some(*node) {
                return Err("interrupted");
            }
            self.visited.push((*node, index, parent, in_document));
            Ok(in_document || *node == 1)
        }
    }
    /// 文書から抜けた兄弟にも到達し、文書所属を持ち越さない。
    #[test]
    fn traverses_all_siblings_and_restores_document_scope() {
        let mut tree = Fixture {
            fail_at: None,
            visited: vec![],
        };
        assert_eq!(walk(&mut tree, 0, 5, 3), Ok(5));
        assert_eq!(
            tree.visited,
            vec![
                (0, 0, None, false),
                (1, 1, Some(0), false),
                (2, 2, Some(1), true),
                (3, 3, Some(1), true),
                (4, 4, Some(0), false)
            ]
        );
    }
    /// プロバイダー失敗や介入を「子なし・完了」に取り違えない。
    #[test]
    fn interruption_propagates_without_visiting_later_candidates() {
        let mut tree = Fixture {
            fail_at: Some(3),
            visited: vec![],
        };
        assert_eq!(walk(&mut tree, 0, 5, 3), Err("interrupted"));
        assert_eq!(
            tree.visited.iter().map(|v| v.0).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    /// 非Edit参照では列挙が完了しても入力欄との一意一致を主張しない。
    #[test]
    fn non_edit_reference_never_confirms_a_unique_input() {
        assert!(!unique(true, false, 1, 0));
        assert!(!unique(true, false, 1, 1));
        assert!(unique(true, true, 1, 1));
    }

    /// 最後の兄弟を見落とす上限と、子を見落とす上限は失敗にする。
    #[test]
    fn partial_traversal_never_becomes_unique() {
        let mut tree = Fixture {
            fail_at: None,
            visited: vec![],
        };
        assert_eq!(walk(&mut tree, 0, 4, 3), Err("node_limit"));
        assert_eq!(walk(&mut tree, 0, 5, 2), Err("depth_limit"));
        assert!(!unique(false, true, 1, 1));
        assert!(!unique(true, true, 2, 1));
        assert!(!unique(true, true, 1, 0));
        assert!(unique(true, true, 1, 1));
    }
    /// ID/Classの生値を戻さず、同値・別値・空・取得不能を区別する。
    #[test]
    fn identity_comparison_returns_only_known_nonempty_equality() {
        let a: Vec<u16> = "private sample\nnot for logs".encode_utf16().collect();
        let b: Vec<u16> = "different sample".encode_utf16().collect();
        assert_eq!(same_nonempty(Some(&a), Some(&a)), Some(true));
        assert_eq!(same_nonempty(Some(&a), Some(&b)), Some(false));
        assert_eq!(same_nonempty(Some(&[]), Some(&[])), Some(false));
        assert_eq!(same_nonempty(Some(&a), Some(&[])), Some(false));
        assert_eq!(same_nonempty(None, Some(&a)), None);
        assert_eq!(same_nonempty(Some(&a), None), None);
        assert_eq!(same_nonempty(None, None), None);
    }
}
