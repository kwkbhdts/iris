use crate::{
    app::{self, Request},
    diagnostic::{App, Trace},
    ime, model, policy, rules, transition, types, writer,
};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use windows::{
    core::Interface,
    Win32::{
        Foundation::HWND,
        System::{Com::*, Variant::VARIANT},
        UI::Accessibility::*,
    },
};
type Result<T> = std::result::Result<T, &'static str>;

struct Apartment;
impl Drop for Apartment {
    /// UIA参照を解放してから専用MTAを閉じる。
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
pub struct Session {
    automation: IUIAutomation2,
    condition: IUIAutomationCondition,
    walker: IUIAutomationTreeWalker,
    _apartment: Apartment,
}
struct Target {
    element: IUIAutomationElement,
    regions: Vec<IUIAutomationElement>,
}
struct Outcome {
    search: &'static str,
    revalidated: bool,
    set_focus: &'static str,

    interrupted: bool,
    permit_epoch: Option<u64>,
    verification_failed: bool,
    input: Option<model::InputReply>,
    manual: bool,
}
impl Default for Outcome {
    /// 未実施と未確認を成功から分離する。
    fn default() -> Self {
        Self {
            search: "not_started",
            revalidated: false,
            set_focus: "not_called",

            interrupted: false,
            permit_epoch: None,
            verification_failed: false,
            input: None,
            manual: false,
        }
    }
}

/// 自由文エラーを出さず、固定API名とHRESULTを記録する。
fn api<T>(trace: &mut Trace, name: &'static str, value: windows::core::Result<T>) -> Result<T> {
    value.map_err(|error| {
        trace.record(format_args!(
            "api={name} hr=0x{:08X}",
            error.code().0 as u32
        ));
        "uia_error"
    })
}
/// 世代はリセットせず、同一プロセスの前面通知でも中断する。
fn guard(request: Request, trace: &mut Trace, outcome: &mut Outcome) -> Result<()> {
    if let Some(epoch) = outcome.permit_epoch {
        if outcome.verification_failed {
            return Err("verification_already_aborted");
        }
        return app::check_read_only(request, epoch).map_err(|reason| {
            outcome.verification_failed = true;
            trace.record(format_args!(
                "verification_aborted=true reason={reason} permit_epoch={epoch}"
            ));
            reason
        });
    }
    app::check(request).map_err(|failure| {
        outcome.interrupted = true;
        app::trace_interventions(request, trace, "original_guard_failed", Some(failure.epoch));
        failure.reason
    })
}

macro_rules! call {
    ($trace:expr, $request:expr, $outcome:expr, $name:literal, $expr:expr) => {{
        guard($request, $trace, $outcome)?;
        api($trace, $name, unsafe { $expr })
    }};
}

impl Session {
    /// 固定のControl View Edit条件を専用MTA内で用意する。
    pub fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|_| "com_init")?;
            let apartment = Apartment;
            let automation: IUIAutomation2 =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                    .map_err(|_| "uia_init")?;
            automation
                .SetConnectionTimeout(200)
                .map_err(|_| "uia_timeout")?;
            automation
                .SetTransactionTimeout(200)
                .map_err(|_| "uia_timeout")?;
            let mut condition = automation
                .ControlViewCondition()
                .map_err(|_| "condition_init")?;
            for (property, value) in [
                (
                    UIA_ControlTypePropertyId,
                    VARIANT::from(UIA_EditControlTypeId.0),
                ),
                (UIA_IsEnabledPropertyId, VARIANT::from(true)),
                (UIA_IsKeyboardFocusablePropertyId, VARIANT::from(true)),
                (UIA_IsOffscreenPropertyId, VARIANT::from(false)),
                (UIA_IsPasswordPropertyId, VARIANT::from(false)),
            ] {
                let part = automation
                    .CreatePropertyCondition(property, &value)
                    .map_err(|_| "condition_init")?;
                condition = automation
                    .CreateAndCondition(&condition, &part)
                    .map_err(|_| "condition_init")?;
            }
            let walker = automation.RawViewWalker().map_err(|_| "walker_init")?;
            Ok(Self {
                automation,
                condition,
                walker,
                _apartment: apartment,
            })
        }
    }

    /// 探索・API応答・観測・中断を独立して報告し、登録は保持しない。
    pub fn execute(&mut self, request: Request) -> String {
        let mut trace = Trace::new(request.context);
        let mut outcome = Outcome::default();
        let result = self.perform(request, &mut trace, &mut outcome);
        if request.started.elapsed().as_millis() >= policy::BUDGET_MS {
            app::STOP.store(true, Ordering::SeqCst);
            if outcome.permit_epoch.is_some() {
                outcome.verification_failed = true;
            } else {
                outcome.interrupted = true;
            }
        }
        trace.stage = "summary";
        let input = outcome.input.unwrap_or(model::InputReply {
            attempted: false,
            inserted: 0,
        });
        trace.record(format_args!("search={} revalidated={} set_focus={} original_interrupted={} verification_aborted={} permit_epoch={:?} input_attempted={} input_inserted={} manual_check={} reason={} app_send_result=unknown", outcome.search, outcome.revalidated, outcome.set_focus, outcome.interrupted, outcome.verification_failed, outcome.permit_epoch, input.attempted, input.inserted, outcome.manual, result.as_ref().err().copied().unwrap_or("none")));
        let label = if input.attempted && input.inserted == model::INPUT_COUNT {
            "INPUT_ENQUEUED"
        } else if input.attempted {
            "INPUT_PARTIAL_OR_FAILED"
        } else {
            "NO_INPUT"
        };
        trace.finish(label, app::STOP.load(Ordering::SeqCst))
    }

    /// 親なしとエラーを区別し、子や兄弟の全走査は行わない。
    fn parent(
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
    fn editor(
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
        Ok(rules::markers(&class)?.editor)
    }

    /// Raw祖先の順序・文書・前面ルートを検証し、領域のUIA参照を保持する。
    fn regions(
        &self,
        node: &IUIAutomationElement,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<Option<Vec<IUIAutomationElement>>> {
        let mut node = node.clone();
        let mut path = rules::Ancestors::default();
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
                    .advance(rules::markers(&class)?, kind == UIA_DocumentControlTypeId)
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

    /// 本文側のフォーカスが同じ前面ルート内のどの文書に属するか確認する。
    fn document(
        &self,
        node: &IUIAutomationElement,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<IUIAutomationElement> {
        let mut node = node.clone();
        let mut document = None;
        for depth in 0..rules::ANCESTOR_LIMIT {
            if call!(
                trace,
                request,
                outcome,
                "CompareElements(initial_root)",
                self.automation.CompareElements(&node, root)
            )?
            .as_bool()
            {
                return document.ok_or("initial_focus_outside_document");
            }
            let kind = call!(
                trace,
                request,
                outcome,
                "ControlType(initial)",
                node.CurrentControlType()
            )?;
            if document.is_none() && kind == UIA_DocumentControlTypeId {
                document = Some(node.clone());
            }
            if depth + 1 == rules::ANCESTOR_LIMIT {
                return Err("initial_ancestor_limit");
            }
            node = self
                .parent(&node, request, trace, outcome)?
                .ok_or("initial_root_not_reached")?;
        }
        Err("initial_ancestor_limit")
    }

    /// UIA条件検索が完了した全候補を調べ、固定ルールの一致が一件だけなら返す。
    fn search(
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
    fn observe(
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

    /// F1ごとに探索し直し、再検証後に一回だけフォーカスを要求する。
    fn perform(&self, request: Request, trace: &mut Trace, outcome: &mut Outcome) -> Result<()> {
        if request.context.app != App::ChatGpt {
            return Err("unsupported_app");
        }
        trace.stage = "search";
        outcome.search = "incomplete";
        let root = call!(
            trace,
            request,
            outcome,
            "ElementFromHandle",
            self.automation
                .ElementFromHandle(HWND(request.window.hwnd as *mut _))
        )?;
        let initial = call!(
            trace,
            request,
            outcome,
            "GetFocusedElement(initial)",
            self.automation.GetFocusedElement()
        )?;
        let initial_document = self.document(&initial, &root, request, trace, outcome)?;
        let target = match self.search(&root, request, trace, outcome) {
            Ok(value) => {
                outcome.search = "unique";
                value
            }
            Err(reason) => {
                outcome.search = match reason {
                    "no_match" => "no_match",
                    "ambiguous" => "ambiguous",
                    _ => "incomplete",
                };
                return Err(reason);
            }
        };
        if !call!(
            trace,
            request,
            outcome,
            "CompareElements(document)",
            self.automation
                .CompareElements(&initial_document, &target.regions[3])
        )?
        .as_bool()
        {
            return Err("different_document");
        }
        trace.stage = "revalidate";
        // 再検索も同じ総期限内。追加候補と領域差し替えを直前に検出する。
        let checked = self.search(&root, request, trace, outcome)?;
        for (before, after) in std::iter::once((&target.element, &checked.element))
            .chain(target.regions.iter().zip(&checked.regions))
        {
            if !call!(
                trace,
                request,
                outcome,
                "CompareElements(revalidate)",
                self.automation.CompareElements(before, after)
            )?
            .as_bool()
            {
                return Err("target_or_region_changed");
            }
        }
        let actual = call!(
            trace,
            request,
            outcome,
            "GetFocusedElement(before)",
            self.automation.GetFocusedElement()
        )?;
        if !call!(
            trace,
            request,
            outcome,
            "CompareElements(initial,before)",
            self.automation.CompareElements(&initial, &actual)
        )?
        .as_bool()
        {
            return Err("initial_focus_changed");
        }
        if !self.editor(&target.element, request, trace, outcome)? {
            return Err("target_no_longer_editable");
        }
        outcome.revalidated = true;
        trace.stage = "focus.set";
        guard(request, trace, outcome)?;
        let started = Instant::now();
        let hr = unsafe { (target.element.vtable().SetFocus)(target.element.as_raw()) };
        let completed = Instant::now();
        outcome.set_focus = if hr.is_ok() {
            "returned_ok"
        } else {
            "returned_error"
        };
        trace.record(format_args!(
            "start_mono_us={} end_mono_us={} hr=0x{:08X}",
            started.duration_since(request.started).as_micros(),
            completed.duration_since(request.started).as_micros(),
            hr.0 as u32
        ));
        api(trace, "SetFocus", hr.ok())?;
        trace.stage = "transition";
        let original_valid = app::check(request).is_ok();
        let (epoch, snapshot) = app::read_only_snapshot(request);
        outcome.interrupted = !original_valid || epoch != request.epoch;
        app::trace_interventions(request, trace, "transition_history", Some(epoch));
        let unchanged = original_valid
            && epoch == request.epoch
            && snapshot.total == Some(0)
            && snapshot.missing == 0;
        let limited = transition::allowed(
            &snapshot,
            request.window.pid,
            request.window.hwnd,
            crate::diagnostic::mono_ns(started),
            crate::diagnostic::mono_ns(completed),
        );
        trace.record(format_args!("original_interrupted={} unchanged={unchanged} limited_transition={limited} attribution=unknown", outcome.interrupted));
        if !unchanged && !limited {
            return Err("transition_rejected");
        }
        // ここから新しい送信前検証。原要求・世代・失効履歴は書き換えない。
        app::check_read_only(request, epoch)?;
        outcome.permit_epoch = Some(epoch);
        trace.record(format_args!(
            "verification_started=true original_epoch={} permit_epoch={epoch}",
            request.epoch
        ));
        trace.stage = "verify.focus";
        let mut matched = false;
        for offset in [0, 50, 150] {
            loop {
                guard(request, trace, outcome)?;
                let wait = policy::observation_wait(
                    request.started.elapsed().as_millis(),
                    completed.elapsed().as_millis(),
                    offset,
                )
                .ok_or("focus_observation_budget")?;
                if wait == 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(wait));
            }
            matched = self.observe(&target.element, request, trace, outcome)?;
            trace.record(format_args!(
                "target_offset_ms={offset} actual_since_set_us={} focus_match={matched}",
                completed.elapsed().as_micros()
            ));
            if matched {
                break;
            }
        }
        if !matched {
            return Err("focus_not_confirmed");
        }
        self.validate_send(&target, &root, request, trace, outcome)?;
        if request.started.elapsed().as_millis() >= policy::BUDGET_MS - model::DEADLINE_MS as u128 {
            return Err("insufficient_ime_budget");
        }
        guard(request, trace, outcome)?;
        trace.stage = "ime.prepare";
        let inner = ime::prepare_auto(types::WindowId {
            hwnd: request.window.hwnd,
            pid: request.window.pid,
            tid: request.window.tid,
        })?;
        guard(request, trace, outcome)?;
        let quick = || app::check_read_only(request, epoch).is_ok();
        let result = {
            let mut validate = || {
                trace.stage = "pre_input.verify";
                match self.validate_send(&target, &root, request, trace, outcome) {
                    Ok(()) => true,
                    Err(reason) => {
                        trace.record(format_args!("send_precheck=false reason={reason}"));
                        false
                    }
                }
            };
            let mut backend = writer::Writer {
                inner,
                quick: &quick,
                validate: &mut validate,
                failed: false,
                consumed: false,
            };
            let result = model::run(&mut backend);
            if backend.failed {
                outcome.verification_failed = true;
            }
            result
        };
        outcome.input = result.input;
        outcome.manual = result.manual;
        trace.stage = "input.result";
        trace.record(format_args!(
            "diagnostic={} ime_status={} manual_check={} app_send_result=unknown",
            result.diagnostic(),
            result.status,
            result.manual
        ));
        if result.manual
            || outcome.verification_failed
            || matches!(
                result.status,
                "original_unknown; no_input_or_change"
                    | "off_not_confirmed_or_guard_failed; no_input"
            )
        {
            app::STOP.store(true, Ordering::SeqCst);
            trace.record(format_args!(
                "restart_required=true no_automatic_retry=true"
            ));
        }
        if outcome.verification_failed {
            return Err("send_verification_failed");
        }
        if result.manual {
            return Err("input_or_restore_requires_manual_check");
        }
        Ok(())
    }

    /// 送信前に候補一意性・領域同一性・実フォーカスを再確認する。
    fn validate_send(
        &self,
        target: &Target,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<()> {
        let checked = self.search(root, request, trace, outcome)?;
        for (before, after) in std::iter::once((&target.element, &checked.element))
            .chain(target.regions.iter().zip(&checked.regions))
        {
            if !call!(
                trace,
                request,
                outcome,
                "CompareElements(send_target)",
                self.automation.CompareElements(before, after)
            )?
            .as_bool()
            {
                outcome.verification_failed = true;
                return Err("send_target_or_region_changed");
            }
        }
        if !self.observe(&target.element, request, trace, outcome)? {
            outcome.verification_failed = true;
            return Err("send_focus_changed");
        }
        guard(request, trace, outcome)
    }
}
