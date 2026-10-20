use super::*;
use crate::restore::{self, Attributes, Probe};

struct SavedTarget<'a> {
    session: &'a Session,
    target: &'a Target,
    root: &'a IUIAutomationElement,
    request: Request,
    trace: &'a mut Trace,
    outcome: &'a mut Outcome,
}
impl Probe for SavedTarget<'_> {
    /// 固定世代と期限を維持する。
    fn check(&mut self) -> Result<()> {
        guard(self.request, self.trace, self.outcome)
    }

    /// 別ルート・公開ダイアログへの復元は行わない。
    fn root(&mut self) -> Result<()> {
        self.session
            .unblocked(self.root, self.request, self.trace, self.outcome)
    }

    /// 元要素が同じルートに残り、ブラウザーではDocument配下か確認する。
    fn ancestry(&mut self) -> Result<()> {
        let document = self.session.ancestor_document(
            &self.target.element,
            self.root,
            self.request,
            self.trace,
            self.outcome,
        )?;
        if selection::requires_document(self.request.context.app) && !document {
            return Err("restore_document_missing");
        }
        Ok(())
    }

    /// 元要素の3属性のみ読み、Enabled・Focusableを復元条件にしない。
    fn attributes(&mut self) -> Result<Attributes> {
        let element = &self.target.element;
        let edit = call!(
            self.trace,
            self.request,
            self.outcome,
            "ControlType(restore)",
            element.CurrentControlType()
        )? == UIA_EditControlTypeId;
        let password = call!(
            self.trace,
            self.request,
            self.outcome,
            "IsPassword(restore)",
            element.CurrentIsPassword()
        )?
        .as_bool();
        let offscreen = call!(
            self.trace,
            self.request,
            self.outcome,
            "IsOffscreen(restore)",
            element.CurrentIsOffscreen()
        )?
        .as_bool();
        self.trace.record(format_args!(
            "restore_saved_edit={edit} password={password} offscreen={offscreen}"
        ));
        Ok(Attributes {
            edit,
            password,
            offscreen,
        })
    }

    /// Win32 HWND内の別欄への移動もUIA同一性で拒否する。
    fn focused(&mut self) -> Result<bool> {
        let same =
            self.session
                .observe(&self.target.element, self.request, self.trace, self.outcome)?;
        self.trace
            .record(format_args!("restore_saved_focus_match={same}"));
        Ok(same)
    }
}
impl Session {
    /// 復元は保存済みの元要素だけを確認し、再選択・フォーカス移動は行わない。
    pub(super) fn validate_restore(
        &self,
        target: &Target,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<()> {
        trace.record(format_args!(
            "restore_scope=saved_target candidate_reselection=false"
        ));
        restore::validate(&mut SavedTarget {
            session: self,
            target,
            root,
            request,
            trace,
            outcome,
        })
    }
}
