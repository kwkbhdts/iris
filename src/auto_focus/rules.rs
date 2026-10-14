use crate::diagnostic::App;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Profile {
    #[default]
    ChatGpt,
    Claude,
}
impl Profile {
    /// 実行ファイルの許可とは別に、対応する構造ルールだけを選ぶ。
    pub fn for_app(app: App) -> Result<Self, &'static str> {
        match app {
            App::ChatGpt => Ok(Self::ChatGpt),
            App::Claude => Ok(Self::Claude),
            _ => Err("unsupported_rule_app"),
        }
    }
}

pub const CLASS_LIMIT: usize = 4096;
pub const CANDIDATE_LIMIT: usize = 24;
pub const ANCESTOR_LIMIT: usize = 64;

#[derive(Default, Clone, Copy)]
pub struct Markers {
    pub editor: bool,
    pub footer: bool,
    pub thread: bool,
    pub messaging: bool,
    pub core: bool,
}

/// クラスを完全な空白区切りトークンとして読み、元文字列は保持しない。
pub fn markers(profile: Profile, value: &[u16]) -> Result<Markers, &'static str> {
    if value.len() > CLASS_LIMIT {
        return Err("class_limit");
    }
    let text = String::from_utf16(value).map_err(|_| "invalid_class_utf16")?;
    let has = |token| text.split_ascii_whitespace().any(|part| part == token);
    Ok(match profile {
        Profile::ChatGpt => Markers {
            editor: has("ProseMirror"),
            footer: has("conversation-footer"),
            thread: has("thread-pane"),
            messaging: has("messaging-root") && has("messaging-embedded"),
            core: false,
        },
        Profile::Claude => Markers {
            editor: has("tiptap") && has("ProseMirror"),
            footer: has("epitaxy-chat-panel"),
            thread: has("tiles-shell"),
            messaging: has("dframe-pane-primary"),
            core: has("chat-ui-core"),
        },
    })
}

#[derive(Default)]
pub struct Ancestors {
    stage: u8,
    profile: Profile,
}
impl Ancestors {
    /// アプリごとの必要な領域順序を要求内に固定する。
    pub fn new(profile: Profile) -> Self {
        Self { stage: 0, profile }
    }

    /// 別々の祖先で順序を満たす。中間要素は許容し、固定深度は要求しない。
    pub fn advance(&mut self, markers: Markers, document: bool) -> Option<usize> {
        let matched = match self.stage {
            0 => markers.footer,
            1 => markers.thread,
            2 => markers.messaging,
            3 if self.profile == Profile::Claude => markers.core,
            3 => document,
            4 if self.profile == Profile::Claude => document,
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
        reached_root
            && self.stage
                == if self.profile == Profile::Claude {
                    5
                } else {
                    4
                }
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
        markers(Profile::ChatGpt, &value.encode_utf16().collect::<Vec<_>>()).unwrap()
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
        assert!(markers(Profile::ChatGpt, &[0xd800]).is_err());
        assert!(markers(Profile::ChatGpt, &vec![65; CLASS_LIMIT + 1]).is_err());
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
    /// 二会話の中間Group増減を許容し、必須の各目印と順序は緩めない。
    #[test]
    fn claude_requires_ordered_distinct_regions_but_not_fixed_depth() {
        let sequence = [
            "epitaxy-chat-panel",
            "tiles-shell",
            "dframe-pane-primary",
            "chat-ui-core",
        ];
        let cm = |value: &str| {
            markers(Profile::Claude, &value.encode_utf16().collect::<Vec<_>>()).unwrap()
        };
        for gaps in [0, 1, 7] {
            let mut path = Ancestors::new(Profile::Claude);
            for token in sequence {
                for _ in 0..gaps {
                    path.advance(cm("layout"), false);
                }
                path.advance(cm(token), false);
            }
            assert!(!path.matches(true));
            assert_eq!(path.advance(cm(""), true), Some(4));
            assert!(!path.matches(false));
            assert!(path.matches(true));
            path.advance(cm(""), true);
            assert!(path.matches(true));
        }
        for omitted in 0..4 {
            let mut path = Ancestors::new(Profile::Claude);
            for (index, token) in sequence.iter().enumerate() {
                if index != omitted {
                    path.advance(cm(token), false);
                }
            }
            path.advance(cm(""), true);
            assert!(!path.matches(true));
        }
        let mut combined = Ancestors::new(Profile::Claude);
        combined.advance(cm(&sequence.join(" ")), true);
        assert!(!combined.matches(true));
        let mut reversed = Ancestors::new(Profile::Claude);
        for token in sequence.iter().rev() {
            reversed.advance(cm(token), false);
        }
        reversed.advance(cm(""), true);
        assert!(!reversed.matches(true));
    }

    /// focused状態・部分一致・別アプリの目印・不正文字列を採用しない。
    #[test]
    fn claude_editor_and_profile_are_strict() {
        let cm = |value: &str| {
            markers(Profile::Claude, &value.encode_utf16().collect::<Vec<_>>()).unwrap()
        };
        assert!(cm("tiptap ProseMirror").editor);
        assert!(cm("tiptap ProseMirror ProseMirror-focused").editor);
        for text in [
            "ProseMirror",
            "tiptap ProseMirror-focused",
            "tiptap2 ProseMirror",
            "tiptap MyProseMirror",
        ] {
            assert!(!cm(text).editor);
        }
        assert!(!cm("epitaxy-chat-panel-extra").footer);
        assert!(!cm("dframe-pane-primary_hash").messaging);
        assert!(!cm("conversation-footer thread-pane messaging-root messaging-embedded").footer);
        assert!(!m("epitaxy-chat-panel tiles-shell dframe-pane-primary chat-ui-core").footer);
        assert!(markers(Profile::Claude, &[0xd800]).is_err());
        assert!(markers(Profile::Claude, &vec![65; CLASS_LIMIT + 1]).is_err());
        assert_eq!(Profile::for_app(App::Claude), Ok(Profile::Claude));
        assert_eq!(Profile::for_app(App::ChatGpt), Ok(Profile::ChatGpt));
        assert!(Profile::for_app(App::Chrome).is_err());
        assert!(Profile::for_app(App::Firefox).is_err());
    }
}
