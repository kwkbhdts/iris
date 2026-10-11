pub const STRING_LIMIT: usize = 128;
pub const PATH_LIMIT: usize = 64;

pub struct Rendered {
    pub quoted: String,
    pub truncated: bool,
    pub invalid_utf16: bool,
}

/// 文字列を有界にし、改行・端末制御・引用符を一行のデータへエスケープする。
pub fn render(value: &[u16]) -> Rendered {
    let mut end = value.len().min(STRING_LIMIT);
    // 上限で有効なサロゲートペアを分断しない。
    if end < value.len()
        && end > 0
        && (0xd800..=0xdbff).contains(&value[end - 1])
        && (0xdc00..=0xdfff).contains(&value[end])
    {
        end -= 1;
    }
    let mut invalid_utf16 = false;
    let text: String = char::decode_utf16(value[..end].iter().copied())
        .map(|value| {
            value.unwrap_or_else(|_| {
                invalid_utf16 = true;
                char::REPLACEMENT_CHARACTER
            })
        })
        .collect();
    Rendered {
        quoted: format!("{text:?}"),
        truncated: end < value.len(),
        invalid_utf16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// メタデータ内の制御文字が新しいログ行や端末命令にならない。
    #[test]
    fn escapes_controls_quotes_and_direction_marks() {
        let value: Vec<_> = "class\n\r\t\0\u{1b}[31m\"\\\u{202e}日本語"
            .encode_utf16()
            .collect();
        let shown = render(&value);
        assert!(!shown
            .quoted
            .contains(['\n', '\r', '\t', '\0', '\u{1b}', '\u{202e}']));
        assert!(shown.quoted.contains("\\n"));
        assert!(shown.quoted.contains("\\\""));
        assert!(shown.quoted.contains("日本語"));
        assert!(!shown.truncated);
        assert!(!shown.invalid_utf16);
    }

    /// 長文の上限とサロゲート境界を明示し、省略を完全な値に見せない。
    #[test]
    fn bounds_utf16_without_splitting_a_pair() {
        let exact = vec![b'a' as u16; STRING_LIMIT];
        assert!(!render(&exact).truncated);
        let mut long = vec![b'a' as u16; STRING_LIMIT - 1];
        long.extend("😀tail".encode_utf16());
        let shown = render(&long);
        assert!(shown.truncated);
        assert!(!shown.invalid_utf16);
        assert_eq!(shown.quoted, format!("{:?}", "a".repeat(STRING_LIMIT - 1)));
    }

    /// 空文字列と不正なUTF16を区別し、取得失敗と混同しない。
    #[test]
    fn distinguishes_empty_and_malformed_values() {
        assert_eq!(render(&[]).quoted, "\"\"");
        let shown = render(&[0xd800]);
        assert!(shown.invalid_utf16);
        assert!(!shown.truncated);
    }
}
