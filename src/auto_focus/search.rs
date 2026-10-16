// 呼出側のガードを各APIの直前と処理区間末に適用する、共通のUIA探索。
use super::*;

impl Session {
    /// 親なしとエラーを区別し、子や兄弟の全走査は行わない。
    pub(super) fn parent(
        &self,
        node: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<Option<IUIAutomationElement>> {
        guard(request, trace, outcome)?;
        let mut raw = std::ptr::null_mut();
        let hr = unsafe {
            (self.walker.vtable().GetParentElement)(self.walker.as_raw(), node.as_raw(), &mut raw)
        };
        let value = if raw.is_null() {
            None
        } else {
            Some(unsafe { IUIAutomationElement::from_raw(raw) })
        };
        api(trace, "GetParentElement", hr.ok())?;
        Ok(value)
    }

    /// 対象の現在属性を再確認し、ClassNameを固定トークンへ変換する。
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
        let control = call!(
            trace,
            request,
            outcome,
            "IsControlElement",
            node.CurrentIsControlElement()
        )?
        .as_bool();
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
        if !control
            || !policy::editable(
                kind == UIA_EditControlTypeId,
                enabled,
                focusable,
                offscreen,
                password,
            )
        {
            return Ok(false);
        }
        let class = call!(
            trace,
            request,
            outcome,
            "ClassName",
            node.CurrentClassName()
        )?;
        Ok(rules::markers(rules::Profile::for_app(request.context.app)?, &class)?.editor)
    }

    /// Raw祖先の順序・文書・前面ルートを検証し、領域のUIA参照を保持する。
    pub(super) fn regions(
        &self,
        node: &IUIAutomationElement,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<Option<Vec<IUIAutomationElement>>> {
        let mut node = node.clone();
        let mut path = rules::Ancestors::new(rules::Profile::for_app(request.context.app)?);
        let mut regions = Vec::new();
        for depth in 0..rules::ANCESTOR_LIMIT {
            let same = call!(
                trace,
                request,
                outcome,
                "CompareElements(root)",
                self.automation.CompareElements(&node, root)
            )?
            .as_bool();
            if same {
                return Ok(path.matches(true).then_some(regions));
            }
            // 入力要素自身は祖先として数えない。
            if depth > 0 {
                let kind = call!(
                    trace,
                    request,
                    outcome,
                    "ControlType(ancestor)",
                    node.CurrentControlType()
                )?;
                let class = call!(
                    trace,
                    request,
                    outcome,
                    "ClassName(ancestor)",
                    node.CurrentClassName()
                )?;
                if path
                    .advance(
                        rules::markers(rules::Profile::for_app(request.context.app)?, &class)?,
                        kind == UIA_DocumentControlTypeId,
                    )
                    .is_some()
                {
                    regions.push(node.clone());
                }
            }
            if depth + 1 == rules::ANCESTOR_LIMIT {
                return Err("ancestor_limit");
            }
            node = self
                .parent(&node, request, trace, outcome)?
                .ok_or("no_parent_before_root")?;
        }
        Err("ancestor_limit")
    }

    /// UIA条件検索が完了した全候補を調べ、固定ルールの一致が一件だけなら返す。
    pub(super) fn search(
        &self,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<Target> {
        let list = call!(
            trace,
            request,
            outcome,
            "FindAll(Edit)",
            root.FindAll(TreeScope_Descendants, &self.condition)
        )?;
        let count = call!(trace, request, outcome, "Length", list.Length())?;
        guard(request, trace, outcome)?;
        trace.record(format_args!("edit_candidates={count}"));
        if count < 0 || count as usize > rules::CANDIDATE_LIMIT {
            return Err("candidate_limit");
        }
        let mut matches = Vec::new();
        for index in 0..count {
            let element = call!(
                trace,
                request,
                outcome,
                "GetElement",
                list.GetElement(index)
            )?;
            let editor = self.editor(&element, request, trace, outcome)?;
            let regions = if editor {
                self.regions(&element, root, request, trace, outcome)?
            } else {
                None
            };
            trace.record(format_args!(
                "candidate={index} editor_token={editor} structure_match={}",
                regions.is_some()
            ));
            if let Some(regions) = regions {
                matches.push(Target { element, regions });
            }
        }
        guard(request, trace, outcome)?;
        trace.record(format_args!(
            "search_complete=true rule_matches={}",
            matches.len()
        ));
        if !rules::unique(true, matches.len()) {
            return Err(if matches.is_empty() {
                "no_match"
            } else {
                "ambiguous"
            });
        }
        Ok(matches.remove(0))
    }

    /// SetFocus後は読み取りだけで一致を観測し、介入後は一切続けない。
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
