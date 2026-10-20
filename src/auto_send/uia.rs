use crate::{
    ancestry,
    app::{self, Request},
    diagnostic::Trace,
    ime, model, policy, selection, transition, types, writer,
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
    blocker_condition: IUIAutomationCondition,
    walker: IUIAutomationTreeWalker,
    _apartment: Apartment,
}
struct Target {
    element: IUIAutomationElement,
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
    post: Option<crate::observation::Recorder>,
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
            post: None,
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
    /// 診断と同じ5条件を専用MTA内で用意する。
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
            let condition =
                crate::candidate_condition::create(&automation).map_err(|_| "condition_init")?;
            // 既知の文字列ではなく、UIAが明示するダイアログ／モーダル状態を使う。
            let dialog = automation
                .CreatePropertyCondition(UIA_IsDialogPropertyId, &VARIANT::from(true))
                .map_err(|_| "blocker_condition_init")?;
            let modal = automation
                .CreatePropertyCondition(UIA_WindowIsModalPropertyId, &VARIANT::from(true))
                .map_err(|_| "blocker_condition_init")?;
            let blocker = automation
                .CreateOrCondition(&dialog, &modal)
                .map_err(|_| "blocker_condition_init")?;
            let onscreen = automation
                .CreatePropertyCondition(UIA_IsOffscreenPropertyId, &VARIANT::from(false))
                .map_err(|_| "blocker_condition_init")?;
            let blocker_condition = automation
                .CreateAndCondition(&blocker, &onscreen)
                .map_err(|_| "blocker_condition_init")?;
            let walker = automation.RawViewWalker().map_err(|_| "walker_init")?;
            Ok(Self {
                automation,
                condition,
                blocker_condition,
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
        if result.is_ok() && !app::STOP.load(Ordering::SeqCst) {
            if let Some(mut post) = outcome.post.take() {
                post.follow(&mut trace);
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
        if !crate::TARGET_SCOPE.allows(request.context.app)
            || !crate::selected_mode().allows(request.context.app)
        {
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
        let target = match self.search_root(&root, request, trace, outcome) {
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
        trace.stage = "revalidate";
        // 同じ総期限内で再探索し、別の唯一候補へ切り替わっていないか確認する。
        self.recheck_root(&root, &target, request, trace, outcome)?;
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
        self.unblocked(&root, request, trace, outcome)?;
        outcome.revalidated = true;
        // 同じ検索・再検証を読み取りだけで試し、書込み経路へ到達させない。
        if crate::inspect_only() {
            guard(request, trace, outcome)?;
            trace.record(format_args!("inspect_only=true candidate_match=true semantic_identity_proven=false no_focus_text_enter_ime_writes=true"));
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
        let saved_identity = inner.identity_snapshot();
        let observe = crate::selected_mode() == crate::mode::Mode::ObserveIme;
        let mut recorder = crate::observation::Recorder::new(
            request,
            permitted,
            saved_identity,
            inner.observation_epoch(),
        );
        let quick = || app::check_send(request, permitted).is_ok();
        let result = {
            let mut validate = |stage| {
                trace.stage = match stage {
                    writer::ValidationStage::Send => "pre_input.verify",
                    writer::ValidationStage::Restore => "pre_restore.verify",
                };
                let validation = match stage {
                    writer::ValidationStage::Send => {
                        self.validate_send(&target, &root, request, trace, outcome)
                    }
                    writer::ValidationStage::Restore => {
                        self.validate_restore(&root, request, trace, outcome, saved_identity)
                    }
                };
                match validation {
                    Ok(()) => true,
                    Err(reason) => {
                        trace.record(format_args!(
                            "target_precheck=false stage={stage:?} reason={reason}"
                        ));
                        if stage == writer::ValidationStage::Restore {
                            self.diagnose_restore_failure(
                                &target,
                                &root,
                                request,
                                permitted,
                                saved_identity,
                                trace,
                            );
                        }
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
            let result = if crate::selected_mode().uses_no_ime() {
                crate::comparison::no_ime(&mut backend)
            } else if observe {
                let mut sample = |phase, open| recorder.phase(phase, open);
                let mut observed = crate::comparison::Observed {
                    inner: backend,
                    sample: &mut sample,
                    failed: false,
                };
                let result = model::run(&mut observed);
                if observed.failed {
                    observed.inner.failed = true;
                }
                backend = observed.inner;
                result
            } else {
                model::run(&mut backend)
            };
            if backend.failed {
                outcome.verification_failed = true;
            }
            result
        };
        if observe {
            recorder.flush(trace);
            if result.status == "input_enqueued; legacy_restored; app_and_tsf_result_unknown"
                && !outcome.verification_failed
            {
                outcome.post = Some(recorder);
            } else {
                trace.stage = "ime.observation_end";
                trace.record(format_args!(
                    "reason=no_confirmed_restore no_delayed_samples=true"
                ));
            }
        }
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

    /// 送信前に候補一意性・要素同一性・実フォーカスを再確認する。
    fn validate_send(
        &self,
        target: &Target,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<()> {
        self.recheck_root(root, target, request, trace, outcome)?;
        // 検索後のスクロールでも、直前の可視性・有効性を取り直す。
        if !self.editor(&target.element, request, trace, outcome)? {
            outcome.verification_failed = true;
            return Err("send_target_no_longer_editable");
        }
        if !self.observe(&target.element, request, trace, outcome)? {
            outcome.verification_failed = true;
            return Err("send_focus_changed");
        }
        self.unblocked(root, request, trace, outcome)?;
        guard(request, trace, outcome)
    }
}

#[path = "search.rs"]
mod search;

#[path = "scope_uia.rs"]
mod scope_uia;

#[path = "restore_uia.rs"]
mod restore_uia;

#[path = "failure_uia.rs"]
mod failure_uia;
