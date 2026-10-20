use super::*;
use crate::{
    failure::{self, Identity},
    restore::{self, Probe},
};

struct SavedContext<'a> {
    session: &'a Session,
    root: &'a IUIAutomationElement,
    request: Request,
    trace: &'a mut Trace,
    outcome: &'a mut Outcome,
    saved: (types::Snapshot, types::WindowId, usize),
}
impl Probe for SavedContext<'_> {
    type Focus = IUIAutomationElement;

    /// 固定世代と期限を維持し、失敗した要求を再開しない。
    fn check(&mut self) -> Result<()> {
        guard(self.request, self.trace, self.outcome)
    }

    /// 元ルートの同一性と公開阻害要素の確認を維持する。
    fn root(&mut self) -> Result<()> {
        self.session
            .unblocked(self.root, self.request, self.trace, self.outcome)
    }

    /// 保存先を差し替えず、Win32入力先・IME所有者・layoutの一致を確認する。
    fn identity(&mut self) -> Result<()> {
        guard(self.request, self.trace, self.outcome)?;
        let (foreground, focus, ime, layout) = ime::read_identity();
        let values = failure::compare(
            Identity {
                foreground: self.saved.0.foreground.ok(),
                focus: self.saved.0.focus.ok(),
                ime: Some(self.saved.1),
                layout: (self.saved.2 != 0).then_some(self.saved.2),
            },
            Identity {
                foreground,
                focus,
                ime,
                layout,
            },
        );
        self.trace.record(format_args!("restore_foreground_same={} restore_win32_focus_same={} restore_ime_window_same={} restore_ime_owner_same={} restore_layout_same={}", failure::label(values[0]), failure::label(values[1]), failure::label(values[2]), failure::label(values[3]), failure::label(values[4])));
        guard(self.request, self.trace, self.outcome)?;
        restore::check_identity(values)
    }

    /// 現在フォーカスを読み、新たな送信候補にはしない。
    fn focus(&mut self) -> Result<Self::Focus> {
        call!(
            self.trace,
            self.request,
            self.outcome,
            "GetFocusedElement(restore)",
            self.session.automation.GetFocusedElement()
        )
    }

    /// 現在フォーカスの元ルート所属だけを確認し、送信用属性は条件にしない。
    fn ancestry(&mut self, focus: &Self::Focus) -> Result<()> {
        self.session
            .ancestor_path(
                focus,
                self.root,
                self.request,
                self.trace,
                self.outcome,
                false,
            )
            .map(|_| ())
    }

    /// 検証中のフォーカス変化を止める。旧送信要素との一致は要求しない。
    fn same_focus(&mut self, focus: &Self::Focus) -> Result<bool> {
        let same = self
            .session
            .observe(focus, self.request, self.trace, self.outcome)?;
        self.trace
            .record(format_args!("restore_current_focus_stable={same}"));
        Ok(same)
    }
}
impl Session {
    /// 保存IME操作先の状態復元として検証し、入力・再フォーカスは行わない。
    pub(super) fn validate_restore(
        &self,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
        saved: (types::Snapshot, types::WindowId, usize),
    ) -> Result<()> {
        trace.record(format_args!("restore_scope=saved_ime_context old_uia_identity_required=false candidate_reselection=false"));
        restore::validate(&mut SavedContext {
            session: self,
            root,
            request,
            trace,
            outcome,
            saved,
        })
    }
}
