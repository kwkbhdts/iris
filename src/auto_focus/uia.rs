use crate::{
    app::{self, Request},
    diagnostic::{App, Trace},
    policy, post_cancel, rules,
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
    immediate: &'static str,
    late: &'static str,
    interrupted: bool,
    post_cancel_observation: &'static str,
    post_cancel_reason: &'static str,
}
impl Default for Outcome {
    /// 未実施と未確認を成功から分離する。
    fn default() -> Self {
        Self {
            search: "not_started",
            revalidated: false,
            set_focus: "not_called",
            immediate: "unknown",
            late: "not_run",
            interrupted: false,
            post_cancel_observation: "not_run",
            post_cancel_reason: "not_applicable",
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
    app::check(request).map_err(|failure| {
        outcome.interrupted = true;
        app::trace_interventions(request, trace, "guard_failed", Some(failure.epoch));
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
            outcome.interrupted = true;
        }
        trace.stage = "summary";
        trace.record(format_args!("search={} revalidated={} set_focus={} immediate_focus={} late_focus={} interrupted={} reason={} post_cancel_observation={} post_cancel_reason={} no_text_enter_ime_writes=true", outcome.search, outcome.revalidated, outcome.set_focus, outcome.immediate, outcome.late, outcome.interrupted, result.as_ref().err().copied().unwrap_or("none"), outcome.post_cancel_observation, outcome.post_cancel_reason));
        let observed = outcome.immediate == "match" || outcome.late == "match";
        let label = if outcome.interrupted {
            "INTERRUPTED"
        } else if result.is_err() {
            "FAILED"
        } else if observed {
            "FOCUS_OBSERVED"
        } else {
            "FOCUS_UNCONFIRMED"
        };
        trace.finish(label, app::STOP.load(Ordering::SeqCst))
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
        // 元の操作結果を固定し、条件付き観測の成否では上書きしない。
        let result = (|| {
            guard(request, trace, outcome)?;
            api(trace, "SetFocus", hr.ok())?;
            self.after_focus(&target.element, completed, request, trace, outcome)
        })();
        if outcome.interrupted && outcome.set_focus == "returned_ok" {
            self.post_cancel(&target.element, completed, request, trace, outcome);
        }
        result
    }

    /// 通常の事後観測。失効した要求では呼び出しを続けない。
    fn after_focus(
        &self,
        target: &IUIAutomationElement,
        completed: Instant,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<()> {
        guard(request, trace, outcome)?;
        trace.stage = "focus.immediate";
        let matched = self.observe(target, request, trace, outcome)?;
        outcome.immediate = if matched { "match" } else { "mismatch" };
        trace.record(format_args!("matches_target={matched}"));
        if matched {
            return Ok(());
        }
        for offset in [50, 150] {
            loop {
                guard(request, trace, outcome)?;
                let wait = policy::observation_wait(
                    request.started.elapsed().as_millis(),
                    completed.elapsed().as_millis(),
                    offset,
                )
                .ok_or("observation_budget")?;
                if wait == 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(wait));
            }
            trace.stage = "focus.late";
            let matched = self.observe(target, request, trace, outcome)?;
            outcome.late = if matched { "match" } else { "mismatch" };
            trace.record(format_args!(
                "target_offset_ms={offset} since_set_ms={} matches_target={matched}",
                completed.elapsed().as_millis()
            ));
            if matched {
                break;
            }
        }
        guard(request, trace, outcome)
    }

    /// 中断と観測世代を固定し、即時不一致の場合だけ遅延観測を追加する。
    fn post_cancel(
        &self,
        target: &IUIAutomationElement,
        set_completed: Instant,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) {
        trace.stage = "post_cancel";
        outcome.post_cancel_observation = "unknown";
        let (epoch, snapshot) = app::read_only_snapshot(request);
        if !post_cancel::foreground_only(&snapshot) {
            outcome.post_cancel_reason = "history_not_foreground_only_or_incomplete";
            return;
        }
        trace.record(format_args!("observation_epoch={epoch} original_epoch={} original_operation=interrupted foreground_events={}", request.epoch, snapshot.events.len()));
        for (sample_index, offset) in [0, 50, 150].into_iter().enumerate() {
            // 全サンプルで同じepochを使い、待機中も原要求の期限を延長しない。
            let result = (|| {
                loop {
                    app::check_read_only(request, epoch)?;
                    let wait = policy::observation_wait(
                        request.started.elapsed().as_millis(),
                        set_completed.elapsed().as_millis(),
                        offset,
                    )
                    .ok_or("observation_budget")?;
                    if wait == 0 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(wait));
                }
                let sample_start_us = set_completed.elapsed().as_micros();
                let mut reader = PostCancelReader {
                    session: self,
                    target,
                    request,
                    epoch,
                    trace,
                };
                let sample = post_cancel::once(&mut reader)?;
                let sample_end_us = set_completed.elapsed().as_micros();
                trace.record(format_args!("sample={sample_index} target_offset_ms={offset} clock=since_SetFocus_return sample_start_us={sample_start_us} sample_end_us={sample_end_us} actual_control_type={} matches_target={} observation_epoch={epoch} original_operation=interrupted", sample.control_type, sample.matched));
                Ok(sample)
            })();
            match result {
                Ok(sample) => {
                    outcome.post_cancel_observation =
                        if sample.matched { "match" } else { "mismatch" };
                    outcome.post_cancel_reason = "observed";
                    // 即時一致なら追加照会なし。不一致なら50/150msをそれぞれ記録する。
                    if sample_index == 0 && sample.matched {
                        break;
                    }
                }
                Err(reason) => {
                    outcome.post_cancel_observation = "unknown";
                    outcome.post_cancel_reason = reason;
                    trace.record(format_args!("sample={sample_index} target_offset_ms={offset} sample_incomplete=true since_set_us={} observation_epoch={epoch} reason={reason}", set_completed.elapsed().as_micros()));
                    break;
                }
            }
        }
        trace.record(format_args!(
            "post_cancel_observation={} reason={} original_operation=interrupted",
            outcome.post_cancel_observation, outcome.post_cancel_reason
        ));
    }
}

struct PostCancelReader<'a> {
    session: &'a Session,
    target: &'a IUIAutomationElement,
    request: Request,
    epoch: u64,
    trace: &'a mut Trace,
}
impl post_cancel::Reader for PostCancelReader<'_> {
    type Element = IUIAutomationElement;
    /// 観測開始時の世代と元の前面・期限・停止状態を検証する。
    fn check(&mut self) -> Result<()> {
        app::check_read_only(self.request, self.epoch)
    }
    /// 各サンプルのフォーカス取得は一回だけで、変更APIは呼ばない。
    fn focused(&mut self) -> Result<Self::Element> {
        api(self.trace, "GetFocusedElement(post_cancel)", unsafe {
            self.session.automation.GetFocusedElement()
        })
    }
    /// 取得した実フォーカスの型だけを読み、内容は取得しない。
    fn control_type(&mut self, element: &Self::Element) -> Result<i32> {
        api(self.trace, "ControlType(post_cancel)", unsafe {
            element.CurrentControlType()
        })
        .map(|kind| kind.0)
    }
    /// 既に選択済みの要素との同一性を一回だけ照会する。
    fn matches(&mut self, element: &Self::Element) -> Result<bool> {
        api(self.trace, "CompareElements(post_cancel)", unsafe {
            self.session
                .automation
                .CompareElements(self.target, element)
        })
        .map(|same| same.as_bool())
    }
}

#[path = "../auto_focus/search.rs"]
mod search;
