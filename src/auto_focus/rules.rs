pub const CLASS_LIMIT: usize = 4096;
pub const CANDIDATE_LIMIT: usize = 24;
pub const ANCESTOR_LIMIT: usize = 64;

#[derive(Default, Clone, Copy)]
pub struct Markers {
    pub editor: bool,
    pub footer: bool,
    pub thread: bool,
    pub messaging: bool,
}

/// クラスを完全な空白区切りトークンとして読み、元文字列は保持しない。
pub fn markers(value: &[u16]) -> Result<Markers, &'static str> {
    if value.len() > CLASS_LIMIT {
        return Err("class_limit");
    }
    let text = String::from_utf16(value).map_err(|_| "invalid_class_utf16")?;
    let has = |token| text.split_ascii_whitespace().any(|part| part == token);
    Ok(Markers {
        editor: has("ProseMirror"),
        footer: has("conversation-footer"),
        thread: has("thread-pane"),
        messaging: has("messaging-root") && has("messaging-embedded"),
    })
}

#[derive(Default)]
pub struct Ancestors {
    stage: u8,
}
impl Ancestors {
    /// 別々の祖先で順序を満たす。中間要素は許容し、固定深度は要求しない。
    pub fn advance(&mut self, markers: Markers, document: bool) -> Option<usize> {
        let matched = match self.stage {
            0 => markers.footer,
            1 => markers.thread,
            2 => markers.messaging,
            3 => document,
            _ => false,
        };
        if matched {
            let slot = self.stage as usize;
            self.stage += 1;
            Some(slot)
        } else {
            None
        }
    }
    /// 文書まで揃っても前面ルートに到達しなければ採用しない。
    pub fn matches(&self, reached_root: bool) -> bool {
        reached_root && self.stage == 4
    }
}

/// 不完全な検索や複数候補を一意に読み替えない。
pub fn unique(complete: bool, matches: usize) -> bool {
    complete && matches == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m(value: &str) -> Markers {
        markers(&value.encode_utf16().collect::<Vec<_>>()).unwrap()
    }
    /// 状態クラス・部分文字列・動的な接尾辞を固定目印と混同しない。
    #[test]
    fn exact_tokens_ignore_focus_state_and_unrelated_classes() {
        assert!(m("ProseMirror").editor);
        assert!(m("ProseMirror ProseMirror-focused").editor);
        assert!(m("x\tProseMirror\nother").editor);
        assert!(!m("ProseMirror-focused").editor);
        assert!(!m("MyProseMirror ProseMirror2").editor);
        assert!(m("conversation-footer translate-x-[anything]").footer);
        assert!(!m("conversation-footer_hash").footer);
        assert!(!m("messaging-root").messaging);
        assert!(m("messaging-embedded messaging-root").messaging);
        assert!(!m("messaging-root messaging-embedded-extra").messaging);
    }
    /// 範囲外・不正な識別属性は黙って不一致にせず、検索全体を失敗させる。
    #[test]
    fn invalid_or_oversized_classes_fail() {
        assert!(markers(&[0xd800]).is_err());
        assert!(markers(&vec![65; CLASS_LIMIT + 1]).is_err());
    }
    /// 深度の変化を許容しながら、領域の順序とルート到達を必須にする。
    #[test]
    fn ordered_distinct_ancestors_and_document_are_required() {
        let mut path = Ancestors::default();
        assert_eq!(path.advance(m("thread-pane"), false), None);
        assert_eq!(path.advance(m("conversation-footer"), false), Some(0));
        assert_eq!(path.advance(m("padding"), false), None);
        assert_eq!(path.advance(m("thread-pane"), false), Some(1));
        assert_eq!(
            path.advance(m("messaging-root messaging-embedded"), false),
            Some(2)
        );
        assert!(!path.matches(true));
        assert_eq!(path.advance(m(""), true), Some(3));
        assert!(!path.matches(false));
        assert!(path.matches(true));
        let mut same_node = Ancestors::default();
        same_node.advance(
            m("conversation-footer thread-pane messaging-root messaging-embedded"),
            true,
        );
        assert!(!same_node.matches(true));
    }
    /// 順序違い・埋め込み印の欠落を、よく似た別領域として拒否する。
    #[test]
    fn rejects_wrong_order_and_missing_embedded_marker() {
        for sequence in [
            [
                "thread-pane",
                "conversation-footer",
                "messaging-root messaging-embedded",
            ],
            ["conversation-footer", "thread-pane", "messaging-root"],
        ] {
            let mut path = Ancestors::default();
            for class in sequence {
                path.advance(m(class), false);
            }
            path.advance(m(""), true);
            assert!(!path.matches(true));
        }
    }

    /// 途中の1件や複数一致は採用しない。
    #[test]
    fn selection_requires_completed_single_match() {
        assert!(!unique(false, 1));
        assert!(!unique(true, 0));
        assert!(!unique(true, 2));
        assert!(unique(true, 1));
    }
}
