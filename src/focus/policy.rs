pub const BUDGET_MS: u128 = 1500;
#[cfg(windows)]
pub const MAX_ANCESTORS: usize = 64;

/// 対象・介入世代・期限が保たれた要求だけを続ける。
pub fn valid(same_window: bool, same_epoch: bool, elapsed_ms: u128) -> bool {
    same_window && same_epoch && elapsed_ms < BUDGET_MS
}

/// 総期限に収まる観測だけを待ち、介入を確認する間隔を5ms以下にする。
pub fn observation_wait(request_ms: u128, since_set_ms: u128, target_ms: u128) -> Option<u64> {
    let budget = BUDGET_MS.checked_sub(request_ms)?;
    let remaining = target_ms.saturating_sub(since_set_ms);
    if budget == 0 || remaining >= budget {
        None
    } else {
        Some(remaining.min(5) as u64)
    }
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

    /// 遅延は SetFocus 完了基準とし、過ぎた目標には追加待機しない。
    #[test]
    fn observation_uses_absolute_offsets_and_short_waits() {
        assert_eq!(observation_wait(20, 2, 50), Some(5));
        assert_eq!(observation_wait(67, 49, 50), Some(1));
        assert_eq!(observation_wait(68, 50, 50), Some(0));
        assert_eq!(observation_wait(168, 150, 150), Some(0));
        assert_eq!(observation_wait(200, 182, 150), Some(0));
    }

    /// 目標時刻が総期限に達する場合も、新しい観測を開始しない。
    #[test]
    fn observation_never_extends_request_budget() {
        assert_eq!(observation_wait(1490, 141, 150), Some(5));
        assert_eq!(observation_wait(1490, 140, 150), None);
        assert_eq!(observation_wait(1500, 200, 150), None);
        assert_eq!(observation_wait(u128::MAX, 200, 150), None);
    }
}
