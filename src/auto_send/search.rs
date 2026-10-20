use super::*;
use crate::ancestry::Tree;

/// 検索結果と候補参照を一回の検索だけに閉じ込める。
pub(super) struct Candidates<'a> {
    pub session: &'a Session,
    pub root: &'a IUIAutomationElement,
    pub request: Request,
    pub trace: &'a mut Trace,
    pub outcome: &'a mut Outcome,
    pub list: Option<IUIAutomationElementArray>,
}

impl selection::Probe for Candidates<'_> {
    type Element = IUIAutomationElement;

    /// 既存の固定世代ガードを再検証にも使う。
    fn check(&mut self) -> Result<()> {
        guard(self.request, self.trace, self.outcome)
    }

    /// 診断と同じ条件で前面ルートの全子孫を数える。
    fn count(&mut self) -> Result<i32> {
        let list = call!(
            self.trace,
            self.request,
            self.outcome,
            "FindAll(eligible_Edit)",
            self.root
                .FindAll(TreeScope_Descendants, &self.session.condition)
        )?;
        let count = call!(
            self.trace,
            self.request,
            self.outcome,
            "Length",
            list.Length()
        )?;
        self.list = Some(list);
        self.trace.record(format_args!(
            "candidate_count={count} document_required={}",
            selection::requires_document(self.request.context.app)
        ));
        Ok(count)
    }

    /// 取得後の属性変化は除外で救済せず、検索全体を中断する。
    fn candidate(&mut self, index: i32) -> Result<Self::Element> {
        let list = self.list.as_ref().ok_or("missing_candidate_list")?;
        let element = call!(
            self.trace,
            self.request,
            self.outcome,
            "GetElement",
            list.GetElement(index)
        )?;
        if !self
            .session
            .editor(&element, self.request, self.trace, self.outcome)?
        {
            return Err("candidate_attributes_changed");
        }
        Ok(element)
    }

    /// 診断と同じ有界祖先探索を使い、unknownをfalseに変えない。
    fn ancestry(&mut self, element: &Self::Element) -> Result<bool> {
        self.session
            .ancestor_document(element, self.root, self.request, self.trace, self.outcome)
    }

    /// 再探索で元の候補と同じ要素であることを確認する。
    fn same(&mut self, before: &Self::Element, after: &Self::Element) -> Result<bool> {
        call!(
            self.trace,
            self.request,
            self.outcome,
            "CompareElements(target)",
            self.session.automation.CompareElements(before, after)
        )
        .map(|v| v.as_bool())
    }

    /// 元の5条件件数とDocument条件適用後の件数を区別する。
    fn completed(&mut self, total: i32, eligible: usize) {
        self.trace.record(format_args!(
            "search_complete=true candidate_count={total} eligible_count={eligible}"
        ));
    }
}

struct Ancestors<'a> {
    session: &'a Session,
    root: &'a IUIAutomationElement,
    request: Request,
    trace: &'a mut Trace,
    outcome: &'a mut Outcome,
}
impl Tree for Ancestors<'_> {
    type Node = IUIAutomationElement;

    /// 祖先探索にも送信段階に応じた介入ガードを適用する。
    fn check(&mut self) -> Result<()> {
        guard(self.request, self.trace, self.outcome)
    }

    /// 前面ルートへの到達を参照同一性で確認する。
    fn is_root(&mut self, node: &Self::Node) -> Result<bool> {
        call!(
            self.trace,
            self.request,
            self.outcome,
            "CompareElements(ancestor_root)",
            self.session.automation.CompareElements(node, self.root)
        )
        .map(|v| v.as_bool())
    }

    /// デスクトップではDocumentを条件にも追加照会にも使わない。
    fn is_document(&mut self, node: &Self::Node) -> Result<bool> {
        if !selection::requires_document(self.request.context.app) {
            return Ok(false);
        }
        call!(
            self.trace,
            self.request,
            self.outcome,
            "ControlType(ancestor)",
            node.CurrentControlType()
        )
        .map(|v| v == UIA_DocumentControlTypeId)
    }

    /// 親なしとAPI失敗を区別し、参照を要求外へ持ち出さない。
    fn parent(&mut self, node: &Self::Node) -> Result<Option<Self::Node>> {
        guard(self.request, self.trace, self.outcome)?;
        let mut raw = std::ptr::null_mut();
        let hr = unsafe {
            (self.session.walker.vtable().GetParentElement)(
                self.session.walker.as_raw(),
                node.as_raw(),
                &mut raw,
            )
        };
        let value = if raw.is_null() {
            None
        } else {
            Some(unsafe { IUIAutomationElement::from_raw(raw) })
        };
        api(self.trace, "GetParentElement", hr.ok())?;
        guard(self.request, self.trace, self.outcome)?;
        Ok(value)
    }
}

impl Session {
    /// 保存済み要素の祖先を、送信選択と復元検証で同じ条件で確認する。
    pub(super) fn ancestor_document(
        &self,
        element: &IUIAutomationElement,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<bool> {
        let observed = ancestry::inspect(
            &mut Ancestors {
                session: self,
                root,
                request,
                trace,
                outcome,
            },
            element.clone(),
        );
        trace.record(format_args!(
            "root_reached={} ancestry_complete={} has_document_ancestor={} reason={} parent_missing_depth={:?}",
            observed.root_reached,
            observed.complete,
            match observed.document {
                Some(_) if !selection::requires_document(request.context.app) => "not_checked",
                Some(true) => "true",
                Some(false) => "false",
                None => "unknown",
            },
            observed.reason,
            observed.parent_missing_depth
        ));
        if !observed.complete || !observed.root_reached {
            return Err(observed.reason);
        }
        observed.document.ok_or("ancestry_unknown")
    }

    /// 現在の5属性だけを確認し、内容・Class・Control Viewは取得しない。
    pub(super) fn editor(
        &self,
        node: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<bool> {
        let kind = call!(
            trace,
            request,
            outcome,
            "ControlType",
            node.CurrentControlType()
        )?;
        let enabled = call!(
            trace,
            request,
            outcome,
            "IsEnabled",
            node.CurrentIsEnabled()
        )?
        .as_bool();
        let focusable = call!(
            trace,
            request,
            outcome,
            "IsKeyboardFocusable",
            node.CurrentIsKeyboardFocusable()
        )?
        .as_bool();
        let offscreen = call!(
            trace,
            request,
            outcome,
            "IsOffscreen",
            node.CurrentIsOffscreen()
        )?
        .as_bool();
        let password = call!(
            trace,
            request,
            outcome,
            "IsPassword",
            node.CurrentIsPassword()
        )?
        .as_bool();
        guard(request, trace, outcome)?;
        Ok(policy::editable(
            kind == UIA_EditControlTypeId,
            enabled,
            focusable,
            offscreen,
            password,
        ))
    }

    /// SetFocus後と入力直前に、実フォーカスが選択対象と一致するか読む。
    pub(super) fn observe(
        &self,
        target: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<bool> {
        let actual = call!(
            trace,
            request,
            outcome,
            "GetFocusedElement(after)",
            self.automation.GetFocusedElement()
        )?;
        let same = call!(
            trace,
            request,
            outcome,
            "CompareElements(after)",
            self.automation.CompareElements(target, &actual)
        )?
        .as_bool();
        guard(request, trace, outcome)?;
        Ok(same)
    }
}
