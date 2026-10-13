use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Unknown,
    Abort,
    EpochInputLast,
    EpochMouseMoveLast,
    EpochFocusEventLast,
    EpochTargetLast,
    EpochUnknown,
    Deadline,
    ForegroundChanged,
    ForegroundUnknown,
    FocusChanged,
    FocusUnknown,
    OtherIris,
    ModifierDown,
    SnapshotUnknown,
    TargetUnconfirmed,
    LayoutUnknown,
    ImeWindowChangedOrUnknown,
    ImeOwnerMismatch,
    LayoutChanged,
    HistoryBlockingEvent,
    HistoryUnknown,
}

impl Reason {
    /// 原因の断定ではなく、既存判定で最初に観測した不成立を表す。
    pub fn code(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Abort => "abort_flag",
            Self::EpochInputLast => "epoch_input_last",
            Self::EpochMouseMoveLast => "epoch_mouse_move_last",
            Self::EpochFocusEventLast => "epoch_focus_event_last",
            Self::EpochTargetLast => "epoch_target_last",
            Self::EpochUnknown => "epoch_unknown",
            Self::Deadline => "deadline",
            Self::ForegroundChanged => "foreground_changed",
            Self::ForegroundUnknown => "foreground_unknown",
            Self::FocusChanged => "focus_changed",
            Self::FocusUnknown => "focus_unknown",
            Self::OtherIris => "other_iris",
            Self::ModifierDown => "modifier_down",
            Self::SnapshotUnknown => "snapshot_unknown",
            Self::TargetUnconfirmed => "target_unconfirmed",
            Self::LayoutUnknown => "layout_unknown",
            Self::ImeWindowChangedOrUnknown => "ime_window_changed_or_unknown",
            Self::ImeOwnerMismatch => "ime_owner_mismatch",
            Self::LayoutChanged => "layout_changed",
            Self::HistoryBlockingEvent => "history_blocking_event",
            Self::HistoryUnknown => "history_unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Evidence {
    pub current: Reason,
    pub wait: Option<Reason>,
    pub mouse_move_allowed: bool,
    pub saw_move_only: bool,
}

impl Evidence {
    pub const UNKNOWN: Self = Self {
        current: Reason::Unknown,
        wait: None,
        mouse_move_allowed: false,
        saw_move_only: false,
    };
}

#[derive(Clone, Copy)]
pub enum EventKind {
    Input,
    MouseMove,
    Focus,
    Target,
}

/// イベント内容は保存せず、最後の世代番号だけを分類別に保持する。
pub struct EpochEvidence {
    input: AtomicU64,
    mouse_move: AtomicU64,
    focus: AtomicU64,
    target: AtomicU64,
}

impl EpochEvidence {
    pub const EMPTY: Self = Self {
        input: AtomicU64::new(0),
        mouse_move: AtomicU64::new(0),
        focus: AtomicU64::new(0),
        target: AtomicU64::new(0),
    };

    /// 元の EPOCH 加算後に発行する。未発行なら読み手は不明を返す。
    pub fn record(&self, epoch: u64, kind: EventKind) {
        let slot = match kind {
            EventKind::Input => &self.input,
            EventKind::MouseMove => &self.mouse_move,
            EventKind::Focus => &self.focus,
            EventKind::Target => &self.target,
        };
        slot.store(epoch, Ordering::SeqCst);
    }

    /// 判定が実際に読んだ世代と一致する分類だけを採用する。
    pub fn observed(&self, epoch: u64) -> Reason {
        if epoch == 0 {
            return Reason::EpochUnknown;
        }
        if self.input.load(Ordering::SeqCst) == epoch {
            return Reason::EpochInputLast;
        }
        if self.mouse_move.load(Ordering::SeqCst) == epoch {
            return Reason::EpochMouseMoveLast;
        }
        if self.focus.load(Ordering::SeqCst) == epoch {
            return Reason::EpochFocusEventLast;
        }
        if self.target.load(Ordering::SeqCst) == epoch {
            return Reason::EpochTargetLast;
        }
        Reason::EpochUnknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 未発行・上書き・ゼロ世代を入力介入と誤認しない。
    #[test]
    fn epoch_evidence_only_labels_the_observed_generation() {
        let evidence = EpochEvidence::EMPTY;
        assert_eq!(evidence.observed(0), Reason::EpochUnknown);
        assert_eq!(evidence.observed(1), Reason::EpochUnknown);
        evidence.record(1, EventKind::Input);
        assert_eq!(evidence.observed(1), Reason::EpochInputLast);
        evidence.record(2, EventKind::Focus);
        assert_eq!(evidence.observed(2), Reason::EpochFocusEventLast);
        evidence.record(3, EventKind::Input);
        assert_eq!(evidence.observed(1), Reason::EpochUnknown);
        evidence.record(4, EventKind::Target);
        assert_eq!(evidence.observed(4), Reason::EpochTargetLast);
        assert_eq!(evidence.observed(5), Reason::EpochUnknown);
    }
}
