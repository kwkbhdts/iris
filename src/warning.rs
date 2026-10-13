#[derive(Clone, Copy)]
pub struct Warning {
    pub code: &'static str,
    pub text: &'static str,
    pub restore: Option<crate::model::RestoreDiagnostic>,
}

impl Warning {
    /// 内容ではなく固定の診断段階だけを警告に添える。
    pub const fn new(code: &'static str, text: &'static str) -> Self {
        Self {
            code,
            text,
            restore: None,
        }
    }

    /// 既存処理が返した診断だけを警告に付ける。
    pub fn with_restore(mut self, restore: Option<crate::model::RestoreDiagnostic>) -> Self {
        self.restore = restore;
        self
    }

    /// ビルド時の識別子以外に、対象アプリの情報は付けない。
    pub fn display(self) -> String {
        format!(
            "{}\n\n診断コード: {}{}\n{}",
            self.text,
            self.code,
            self.restore
                .map_or_else(String::new, |r| format!("\n診断詳細: {}", r.display())),
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

    /// I106 のサブコード・投入済みとビルドIDを同じ警告へ表示する。
    #[test]
    fn restore_detail_is_visible_without_claiming_application_success() {
        use crate::model::{guard, InputReply, RestoreDiagnostic, RestoreStage};
        let detail = RestoreDiagnostic {
            stage: RestoreStage::BeforeGuard,
            evidence: guard::Evidence {
                current: guard::Reason::EpochInputLast,
                wait: None,
            },
            reply: None,
            input: Some(InputReply {
                attempted: true,
                inserted: 6,
            }),
        };
        let text = Warning::new("IRIS-I106", "確認不能")
            .with_restore(Some(detail))
            .display();
        for expected in [
            "IRIS-I106-PRE",
            "epoch_input_last",
            "input=enqueued=6/6",
            "app_send_result=unknown",
            crate::build_info::ID,
        ] {
            assert!(text.contains(expected));
        }
    }
}
