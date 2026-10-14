use crate::{
    app::{self, Request},
    diagnostic::Trace,
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
    permitted: Option<[u64; 2]>,
    verifying: bool,
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
            permitted: None,
            verifying: false,
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
/// F1基準を保持し、検証済み二通知以外の介入で中断する。
fn guard(request: Request, trace: &mut Trace, outcome: &mut Outcome) -> Result<()> {
    if outcome.verifying {
        if outcome.verification_failed {
            return Err("verification_already_aborted");
        }
        return app::check_send(request, outcome.permitted).map_err(|reason| {
            outcome.verification_failed = true;
            trace.record(format_args!(
                "verification_aborted=true reason={reason} fixed_start_epoch={}",
                request.epoch
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
            if outcome.verifying {
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
        trace.record(format_args!("search={} revalidated={} set_focus={} original_interrupted={} verification_aborted={} permitted_notifications={:?} input_attempted={} input_inserted={} manual_check={} reason={} app_send_result=unknown", outcome.search, outcome.revalidated, outcome.set_focus, outcome.interrupted, outcome.verification_failed, outcome.permitted, input.attempted, input.inserted, outcome.manual, result.as_ref().err().copied().unwrap_or("none")));
        let label = if crate::inspect_only() {
            if result.is_ok() {
                "OBSERVED"
            } else {
                "INCOMPLETE"
            }
        } else if input.attempted && input.inserted == model::INPUT_COUNT {
            "INPUT_ENQUEUED"
        } else if input.attempted {
            "INPUT_PARTIAL_OR_FAILED"
        } else {
            "NO_INPUT"
        };
        trace.finish(label, app::STOP.load(Ordering::SeqCst))
    }

    /// F1ごとに探索し直し、再検証後に一回だけフォーカスを要求する。
    fn perform(&self, request: Request, trace: &mut Trace, outcome: &mut Outcome) -> Result<()> {
        if !crate::TARGET_SCOPE.allows(request.context.app) {
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
            self.automation.CompareElements(
                &initial_document,
                target.regions.last().ok_or("missing_document_region")?
            )
        )?
        .as_bool()
        {
            return Err("different_document");
        }
        trace.stage = "revalidate";
        // 再検索も同じ総期限内。追加候補と領域差し替えを直前に検出する。
        let checked = self.search(&root, request, trace, outcome)?;
        if target.regions.len() != checked.regions.len() {
            return Err("region_count_changed");
        }
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
        // 同じ検索・再検証を読み取りだけで試し、書込み経路へ到達させない。
        if crate::inspect_only() {
            guard(request, trace, outcome)?;
            trace.record(format_args!("inspect_only=true provisional_rule_match=true semantic_identity_proven=false no_focus_text_enter_ime_writes=true"));
            return Ok(());
        }
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
        outcome.interrupted = !original_valid;
        app::trace_interventions(request, trace, "transition_history", Some(epoch));
        let unchanged = app::check_send(request, None).is_ok();
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
        if !unchanged {
            let mut notifications = snapshot
                .events
                .iter()
                .filter(|e| e.observation.kind == crate::intervention::Kind::Foreground);
            outcome.permitted = Some([
                notifications.next().ok_or("transition_missing")?.epoch,
                notifications.next().ok_or("transition_missing")?.epoch,
            ]);
        }
        app::check_send(request, outcome.permitted)?;
        outcome.verifying = true;
        trace.record(format_args!(
            "verification_started=true fixed_start_epoch={} observed_epoch={epoch}",
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
        let permitted = outcome.permitted;
        let quick = || app::check_send(request, permitted).is_ok();
        let result = {
            let mut validate = |stage| {
                trace.stage = match stage {
                    writer::ValidationStage::Send => "pre_input.verify",
                    writer::ValidationStage::Restore => "pre_restore.verify",
                };
                match self.validate_send(&target, &root, request, trace, outcome) {
                    Ok(()) => true,
                    Err(reason) => {
                        trace.record(format_args!(
                            "target_precheck=false stage={stage:?} reason={reason}"
                        ));
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
                evidence: model::guard::Evidence::UNKNOWN,
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
        if let Some(detail) = result.restore {
            trace.record(format_args!("{}", detail.display()));
        }
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
        if target.regions.len() != checked.regions.len() {
            return Err("region_count_changed");
        }
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
        // 検索後のスクロールでも、直前の可視性・有効性を取り直す。
        if !self.editor(&target.element, request, trace, outcome)? {
            outcome.verification_failed = true;
            return Err("send_target_no_longer_editable");
        }
        if !self.observe(&target.element, request, trace, outcome)? {
            outcome.verification_failed = true;
            return Err("send_focus_changed");
        }
        guard(request, trace, outcome)
    }
}

#[path = "../auto_focus/search.rs"]
mod search;
