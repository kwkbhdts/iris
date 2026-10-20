use crate::{
    ancestry::{self, Observation, Tree},
    app::{self, Request},
    diagnostic::Trace,
    policy,
    report::Report,
};
use std::sync::atomic::Ordering;
use windows::core::Interface;
use windows::Win32::{
    Foundation::HWND,
    System::{Com::*, Variant::VARIANT},
    UI::Accessibility::*,
};

type Result<T> = std::result::Result<T, &'static str>;

struct Apartment;
impl Drop for Apartment {
    /// 専用MTAのUIA参照を破棄してからCOMを終了する。
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
pub struct Session {
    automation: IUIAutomation2,
    candidate_condition: IUIAutomationCondition,
    blocker_condition: IUIAutomationCondition,
    _apartment: Apartment,
}

/// 自由文エラーは読まず、固定API名とHRESULTだけを記録する。
fn api<T>(trace: &mut Trace, name: &'static str, value: windows::core::Result<T>) -> Result<T> {
    value.map_err(|error| {
        trace.record(format_args!(
            "api={name} hr=0x{:08X}",
            error.code().0 as u32
        ));
        "uia_error"
    })
}

/// 前面・介入・期限の確認に既存の読み取り診断ガードを使う。
fn guard(request: Request, trace: &mut Trace) -> Result<()> {
    app::check(request).map_err(|failure| {
        app::trace_interventions(request, trace, "guard_failed", Some(failure.epoch));
        failure.reason
    })
}
macro_rules! read {
    ($trace:expr, $request:expr, $name:literal, $expr:expr) => {{
        guard($request, $trace)?;
        api($trace, $name, unsafe { $expr })
    }};
}

impl Session {
    /// Control Viewを加えず、依頼された5条件だけを検索条件にする。
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
            let mut candidate_condition = automation
                .CreatePropertyCondition(
                    UIA_ControlTypePropertyId,
                    &VARIANT::from(UIA_EditControlTypeId.0),
                )
                .map_err(|_| "condition_init")?;
            for (property, value) in [
                (UIA_IsOffscreenPropertyId, false),
                (UIA_IsEnabledPropertyId, true),
                (UIA_IsKeyboardFocusablePropertyId, true),
                (UIA_IsPasswordPropertyId, false),
            ] {
                let part = automation
                    .CreatePropertyCondition(property, &VARIANT::from(value))
                    .map_err(|_| "condition_init")?;
                candidate_condition = automation
                    .CreateAndCondition(&candidate_condition, &part)
                    .map_err(|_| "condition_init")?;
            }
            let dialog = automation
                .CreatePropertyCondition(UIA_IsDialogPropertyId, &VARIANT::from(true))
                .map_err(|_| "condition_init")?;
            let modal = automation
                .CreatePropertyCondition(UIA_WindowIsModalPropertyId, &VARIANT::from(true))
                .map_err(|_| "condition_init")?;
            let either = automation
                .CreateOrCondition(&dialog, &modal)
                .map_err(|_| "condition_init")?;
            let onscreen = automation
                .CreatePropertyCondition(UIA_IsOffscreenPropertyId, &VARIANT::from(false))
                .map_err(|_| "condition_init")?;
            let blocker_condition = automation
                .CreateAndCondition(&either, &onscreen)
                .map_err(|_| "condition_init")?;
            Ok(Self {
                automation,
                candidate_condition,
                blocker_condition,
                _apartment: apartment,
            })
        }
    }

    /// 失敗時の途中件数も完全な結果と分離し、書込みなしで報告する。
    pub fn execute(&mut self, request: Request) -> String {
        let mut trace = Trace::new(request.context);
        let mut report = Report::default();
        let result = self.inspect(request, &mut trace, &mut report);
        report.complete = result.is_ok();
        if request.started.elapsed().as_millis() >= policy::BUDGET_MS {
            report.complete = false;
            app::STOP.store(true, Ordering::SeqCst);
        }
        let mut details = match result.as_ref() {
            Ok((root, list)) if report.complete => self.details(
                root,
                list,
                report.candidates.unwrap_or(0),
                request,
                &mut trace,
            ),
            _ => Err("base_incomplete"),
        };
        if request.started.elapsed().as_millis() >= policy::BUDGET_MS {
            app::STOP.store(true, Ordering::SeqCst);
            details = Err("deadline_exceeded");
        }
        trace.stage = "summary";
        trace.record(format_args!(
            "details_complete={} details_reason={}",
            details.is_ok(),
            details.err().unwrap_or("none")
        ));
        trace.record(format_args!("complete={} candidate_count={:?} candidate_state={} visible_dialog_or_modal_max={:?} target_eligible={} reason={} unreported_overlay_state=unknown no_focus_text_enter_ime_writes=true", report.complete, report.candidates, report.state(), report.blockers, report.eligible(), result.err().unwrap_or(if report.complete { "none" } else { "deadline" })));
        trace.finish(
            if report.complete && details.is_ok() {
                "OBSERVED"
            } else {
                "INCOMPLETE"
            },
            app::STOP.load(Ordering::SeqCst),
        )
    }

    /// 名前や値・候補要素を取得せず、公開された阻害要素の件数だけを読む。
    fn blockers(
        &self,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        report: &mut Report,
    ) -> Result<()> {
        let list = read!(
            trace,
            request,
            "FindAll(dialog_or_modal)",
            root.FindAll(TreeScope_Subtree, &self.blocker_condition)
        )?;
        let count = read!(trace, request, "Length(dialog_or_modal)", list.Length())?;
        guard(request, trace)?;
        report.observe_blockers(count)
    }

    /// 初期フォーカスによらず、前面の最上位ルート配下を全件数える。
    fn inspect(
        &self,
        request: Request,
        trace: &mut Trace,
        report: &mut Report,
    ) -> Result<(IUIAutomationElement, IUIAutomationElementArray)> {
        if !crate::TARGET_SCOPE.allows(request.context.app) {
            return Err("unsupported_app");
        }
        guard(request, trace)?;
        let hwnd = request.window.hwnd as windows_sys::Win32::Foundation::HWND;
        if unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetAncestor(
                hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::GA_ROOT,
            )
        } != hwnd
        {
            return Err("foreground_not_top_level");
        }
        trace.stage = "root";
        let root = read!(
            trace,
            request,
            "ElementFromHandle",
            self.automation
                .ElementFromHandle(HWND(request.window.hwnd as *mut _))
        )?;
        self.blockers(&root, request, trace, report)?;
        trace.stage = "candidates";
        let list = read!(
            trace,
            request,
            "FindAll(eligible_Edit)",
            root.FindAll(TreeScope_Descendants, &self.candidate_condition)
        )?;
        let count = read!(trace, request, "Length(eligible_Edit)", list.Length())?;
        if count < 0 {
            return Err("invalid_candidate_count");
        }
        report.candidates = Some(count);
        guard(request, trace)?;
        trace.stage = "final_check";
        self.blockers(&root, request, trace, report)?;
        let current = read!(
            trace,
            request,
            "ElementFromHandle(final)",
            self.automation
                .ElementFromHandle(HWND(request.window.hwnd as *mut _))
        )?;
        if !read!(
            trace,
            request,
            "CompareElements(root)",
            self.automation.CompareElements(&root, &current)
        )?
        .as_bool()
        {
            return Err("root_changed");
        }
        guard(request, trace)?;
        Ok((root, list))
    }

    /// 件数判定を変更せず、各候補の祖先だけを追加観測する。
    fn details(
        &self,
        root: &IUIAutomationElement,
        list: &IUIAutomationElementArray,
        count: i32,
        request: Request,
        trace: &mut Trace,
    ) -> Result<()> {
        trace.stage = "ancestry";
        let walker = read!(
            trace,
            request,
            "RawViewWalker",
            self.automation.RawViewWalker()
        )?;
        let mut failure = None;
        for index in 0..count.min(ancestry::MAX_CANDIDATES) {
            let observation = match read!(
                trace,
                request,
                "GetElement(candidate)",
                list.GetElement(index)
            ) {
                Ok(candidate) => ancestry::inspect(
                    &mut Ancestors {
                        session: self,
                        root,
                        walker: &walker,
                        request,
                        trace,
                    },
                    candidate,
                ),
                Err(reason) => Observation::unknown(reason),
            };
            trace.record(format_args!("candidate_index={} root_reached={} ancestry_complete={} has_document_ancestor={} reason={}", index, observation.root_reached, observation.complete, match observation.document { Some(true) => "true", Some(false) => "false", None => "unknown" }, observation.reason));
            if !observation.complete {
                failure.get_or_insert(observation.reason);
            }
            guard(request, trace)?;
        }
        if count > ancestry::MAX_CANDIDATES {
            trace.record(format_args!(
                "candidate_details_omitted={}",
                count - ancestry::MAX_CANDIDATES
            ));
            failure.get_or_insert("candidate_details_limit");
        }
        let current = read!(
            trace,
            request,
            "ElementFromHandle(details_final)",
            self.automation
                .ElementFromHandle(HWND(request.window.hwnd as *mut _))
        )?;
        if !read!(
            trace,
            request,
            "CompareElements(details_root)",
            self.automation.CompareElements(root, &current)
        )?
        .as_bool()
        {
            return Err("root_changed");
        }
        guard(request, trace)?;
        failure.map_or(Ok(()), Err)
    }
}

struct Ancestors<'a> {
    session: &'a Session,
    root: &'a IUIAutomationElement,
    walker: &'a IUIAutomationTreeWalker,
    request: Request,
    trace: &'a mut Trace,
}

impl Tree for Ancestors<'_> {
    type Node = IUIAutomationElement;

    /// 各読み取りの前後で要求の有効性を確認する。
    fn check(&mut self) -> Result<()> {
        guard(self.request, self.trace)
    }

    /// 保存した前面ルートとのUIA同一性だけを比較する。
    fn is_root(&mut self, node: &Self::Node) -> Result<bool> {
        read!(
            self.trace,
            self.request,
            "CompareElements(ancestor_root)",
            self.session.automation.CompareElements(node, self.root)
        )
        .map(|v| v.as_bool())
    }

    /// 内容属性は読まず、コントロール種別だけを確認する。
    fn is_document(&mut self, node: &Self::Node) -> Result<bool> {
        read!(
            self.trace,
            self.request,
            "CurrentControlType(ancestor)",
            node.CurrentControlType()
        )
        .map(|v| v == UIA_DocumentControlTypeId)
    }

    /// S_OKの親なしとAPI失敗を分け、参照は要求内だけで保持する。
    fn parent(&mut self, node: &Self::Node) -> Result<Option<Self::Node>> {
        guard(self.request, self.trace)?;
        let mut raw = std::ptr::null_mut();
        let hr = unsafe {
            (self.walker.vtable().GetParentElement)(self.walker.as_raw(), node.as_raw(), &mut raw)
        };
        let parent = if raw.is_null() {
            None
        } else {
            Some(unsafe { IUIAutomationElement::from_raw(raw) })
        };
        api(self.trace, "GetParentElement", hr.ok())?;
        guard(self.request, self.trace)?;
        Ok(parent)
    }
}
