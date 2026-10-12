#[derive(Clone, Copy)]
pub struct Warning {
    pub code: &'static str,
    pub text: &'static str,
}

impl Warning {
    /// 内容ではなく固定の診断段階だけを警告に添える。
    pub const fn new(code: &'static str, text: &'static str) -> Self {
        Self { code, text }
    }

    /// ビルド時の識別子以外に、対象アプリの情報は付けない。
    pub fn display(self) -> String {
        format!(
            "{}\n\n診断コード: {}\n{}",
            self.text,
            self.code,
            crate::build_info::ID
        )
    }
}

/// UTF-16 の境界を保って通知領域の固定長文字列へ収める。
pub fn balloon(text: &str) -> [u16; 256] {
    let mut result = [0; 256];
    let mut used = 0;
    for ch in text.chars() {
        let mut buffer = [0; 2];
        let encoded = ch.encode_utf16(&mut buffer);
        if used + encoded.len() > 255 {
            break;
        }
        result[used..used + encoded.len()].copy_from_slice(encoded);
        used += encoded.len();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 警告にはコードと同一ビルドIDだけを追加する。
    #[test]
    fn diagnostic_display_contains_build_identity() {
        let text = Warning::new("IRIS-W002", "確認不能").display();
        assert!(text.contains("IRIS-W002"));
        assert!(text.contains(crate::build_info::ID));
    }

    /// 固定長通知でサロゲートを分断せず終端も残す。
    #[test]
    fn balloon_preserves_utf16_boundary() {
        let text = format!("{}🌸", "x".repeat(254));
        let result = balloon(&text);
        assert_eq!(result[254], 0);
        assert_eq!(result[255], 0);
    }
}
