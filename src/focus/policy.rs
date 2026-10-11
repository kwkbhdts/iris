pub const BUDGET_MS: u128 = 1500;
#[cfg(windows)]
pub const MAX_ANCESTORS: usize = 32;

/// 対象・介入世代・期限が保たれた要求だけを続ける。
pub fn valid(same_window: bool, same_epoch: bool, elapsed_ms: u128) -> bool {
    same_window && same_epoch && elapsed_ms < BUDGET_MS
}

/// UIA の Edit として公開された、操作可能な非パスワード欄だけを受け付ける。
pub fn editable(
    edit: bool,
    enabled: bool,
    focusable: bool,
    offscreen: bool,
    password: bool,
) -> bool {
    edit && enabled && focusable && !offscreen && !password
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 前面の復帰でも介入履歴は取り消さず、期限到達は失敗にする。
    #[test]
    fn guards_fail_closed() {
        assert!(valid(true, true, BUDGET_MS - 1));
        assert!(!valid(false, true, 0));
        assert!(!valid(true, false, 0));
        assert!(!valid(true, true, BUDGET_MS));
        assert!(!valid(true, true, u128::MAX));
    }

    /// 各属性が一つでも条件を外れれば登録しない。
    #[test]
    fn reject_non_editable_or_hidden_fields() {
        assert!(editable(true, true, true, false, false));
        for args in [
            (false, true, true, false, false),
            (true, false, true, false, false),
            (true, true, false, false, false),
            (true, true, true, true, false),
            (true, true, true, false, true),
        ] {
            assert!(!editable(args.0, args.1, args.2, args.3, args.4));
        }
    }
}
