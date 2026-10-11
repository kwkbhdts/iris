use crate::{
    app::{self, Request, Window},
    diagnostic::Trace,
    policy,
};
use std::{
    ptr::null_mut,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use windows::{
    core::Interface,
    Win32::{Foundation::HWND, System::Com::*, UI::Accessibility::*},
};

type Result<T> = std::result::Result<T, &'static str>;
const RELATION_DEPTH: usize = 8;
struct Apartment;
impl Drop for Apartment {
    /// COM 要素を解放し終えてから MTA を終了する。
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
struct Registration {
    id: u64,
    window: Window,
    element: IUIAutomationElement,
    document: IUIAutomationElement,
}
pub struct Session {
    automation: IUIAutomation2,
    registration: Option<Registration>,
    _apartment: Apartment,
}

/// 自由文エラーを取得せず、API 名と数値 HRESULT だけを残す。
fn api<T>(trace: &mut Trace, name: &'static str, result: windows::core::Result<T>) -> Result<T> {
    result.map_err(|error| {
        trace.record(format_args!(
            "api={name} hr=0x{:08X} result=error",
            error.code().0 as u32
        ));
        "uia_error"
    })
}

/// 初期化にも同じ固定形式の数値エラーを用いる。
fn init<T>(name: &'static str, result: windows::core::Result<T>) -> Result<T> {
    result.map_err(|error| {
        eprintln!(
            "req=0 key=- app=- stage=init ms=0 api={name} hr=0x{:08X} result=error",
            error.code().0 as u32
        );
        "initialization_failed"
    })
}

/// 追加診断の前にも既存の前面・介入・期限条件を守る。
fn guard(trace: &mut Trace, request: Request) -> Result<()> {
    app::check(request).inspect_err(|reason| {
        trace.record(format_args!("result=guard_failed reason={reason}"));
    })
}

// UIA 呼び出し直前の確認を省略せず、失敗時には呼び出し名を付ける。
macro_rules! call {
    ($trace:expr, $request:expr, $name:literal, $operation:expr) => {{
        guard($trace, $request)?;
        api($trace, $name, $operation)
    }};
}

/// S_OK + NULL（親なし）を Rust バインディング由来のエラーと区別する。
unsafe fn parent(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    trace: &mut Trace,
    request: Request,
) -> Result<Option<IUIAutomationElement>> {
    guard(trace, request)?;
    let mut raw = null_mut();
    let hr = (walker.vtable().GetParentElement)(walker.as_raw(), element.as_raw(), &mut raw);
    api(trace, "GetParentElement", hr.ok())?;
    if raw.is_null() {
        trace.record(format_args!(
            "api=GetParentElement hr=0x{:08X} result=no_parent",
            hr.0 as u32
        ));
        Ok(None)
    } else {
        Ok(Some(IUIAutomationElement::from_raw(raw)))
    }
}

impl Session {
    /// タイムアウトを持つ UIA を同じ専用スレッド内に作る。
    pub fn new() -> Result<Self> {
        unsafe {
            init(
                "CoInitializeEx",
                CoInitializeEx(None, COINIT_MULTITHREADED).ok(),
            )?;
            let apartment = Apartment;
            let automation: IUIAutomation2 = init(
                "CoCreateInstance",
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER),
            )?;
            init("SetConnectionTimeout", automation.SetConnectionTimeout(200))?;
            init(
                "SetTransactionTimeout",
                automation.SetTransactionTimeout(200),
            )?;
            Ok(Self {
                automation,
                registration: None,
                _apartment: apartment,
            })
        }
    }

    /// 一件の診断をまとめ、失敗した登録の解除理由も同じ要求番号で残す。
    pub fn execute(&mut self, request: Request) -> String {
        let mut trace = Trace::new(request.context);
        trace.record(format_args!(
            "registration={:?}",
            self.registration.as_ref().map(|r| r.id)
        ));
        let result = unsafe { self.perform(request, &mut trace) };
        if let Err(reason) = result {
            let failed_stage = trace.stage;
            trace.record(format_args!("result=failed reason={reason}"));
            trace.stage = "registration.clear";
            trace.record(format_args!(
                "registration={:?} reason={reason} failed_stage={failed_stage}",
                self.registration.as_ref().map(|r| r.id)
            ));
            self.registration = None;
        }
        if request.started.elapsed().as_millis() >= policy::BUDGET_MS {
            app::STOP.store(true, Ordering::SeqCst);
        }
        let stopped = app::STOP.load(Ordering::SeqCst);
        trace.finish(
            if stopped {
                "STOPPED"
            } else {
                result.unwrap_or("FAILED")
            },
            stopped,
        )
    }

    /// 入力内容を読まず、拒否に関係する各属性を個別に記録する。
    unsafe fn validate_edit(
        &self,
        element: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
    ) -> Result<()> {
        let kind = call!(
            trace,
            request,
            "CurrentControlType",
            element.CurrentControlType()
        )?;
        trace.record(format_args!(
            "control_type={} edit={}",
            kind.0,
            kind == UIA_EditControlTypeId
        ));
        let enabled = call!(
            trace,
            request,
            "CurrentIsEnabled",
            element.CurrentIsEnabled()
        )?
        .as_bool();
        trace.record(format_args!("enabled={enabled}"));
        let focusable = call!(
            trace,
            request,
            "CurrentIsKeyboardFocusable",
            element.CurrentIsKeyboardFocusable()
        )?
        .as_bool();
        trace.record(format_args!("focusable={focusable}"));
        let offscreen = call!(
            trace,
            request,
            "CurrentIsOffscreen",
            element.CurrentIsOffscreen()
        )?
        .as_bool();
        trace.record(format_args!("offscreen={offscreen}"));
        let password = call!(
            trace,
            request,
            "CurrentIsPassword",
            element.CurrentIsPassword()
        )?
        .as_bool();
        trace.record(format_args!("password={password}"));
        if !policy::editable(
            kind == UIA_EditControlTypeId,
            enabled,
            focusable,
            offscreen,
            password,
        ) {
            return Err("edit_requirements_rejected");
        }
        guard(trace, request)
    }

    /// 最大64ノードまで探索し、ルート自身を Document に数えない条件も変えない。
    unsafe fn document(
        &self,
        element: &IUIAutomationElement,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
    ) -> Result<IUIAutomationElement> {
        let walker = call!(
            trace,
            request,
            "RawViewWalker",
            self.automation.RawViewWalker()
        )?;
        let mut node = element.clone();
        let mut document = None;
        let mut document_depth = None;
        for depth in 0..policy::MAX_ANCESTORS {
            trace.record(format_args!("visit_depth={depth}"));
            let root_match = call!(
                trace,
                request,
                "CompareElements(root)",
                self.automation.CompareElements(&node, root)
            )?
            .as_bool();
            let kind_result = call!(
                trace,
                request,
                "CurrentControlType(ancestor)",
                node.CurrentControlType()
            );
            let kind = kind_result.as_ref().ok().map(|value| value.0);
            trace.record(format_args!("depth={depth} control_type={kind:?} root_match={root_match} document_depth={document_depth:?}"));
            if root_match {
                trace.record(format_args!("walk_end=root_reached root_document_excluded=true document_depth={document_depth:?}"));
                return document.ok_or("root_reached_without_document");
            }
            // 従来必要だった型取得だけを必須にし、追加診断の型取得失敗で判定を変えない。
            if document.is_none() && kind_result? == UIA_DocumentControlTypeId {
                document = Some(node.clone());
                document_depth = Some(depth);
                trace.record(format_args!("document_selected_depth={depth}"));
            }
            node = match parent(&walker, &node, trace, request)? {
                Some(value) => value,
                None => {
                    trace.record(format_args!(
                        "walk_end=no_parent depth={depth} root_match=false"
                    ));
                    return Err("no_parent_before_root");
                }
            };
        }
        trace.record(format_args!(
            "walk_end=depth_limit visited={} root_match=false",
            policy::MAX_ANCESTORS
        ));
        Err("ancestor_limit_reached")
    }

    /// 不一致時だけ既取得要素の親を有界に調べ、成功条件には使用しない。
    unsafe fn ancestor_relation(
        &self,
        descendant: &IUIAutomationElement,
        ancestor: &IUIAutomationElement,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
    ) -> Result<bool> {
        let walker = call!(
            trace,
            request,
            "RawViewWalker(relation)",
            self.automation.RawViewWalker()
        )?;
        let mut node = descendant.clone();
        for depth in 0..RELATION_DEPTH {
            if call!(
                trace,
                request,
                "CompareElements(relation.root)",
                self.automation.CompareElements(&node, root)
            )?
            .as_bool()
            {
                return Ok(false);
            }
            node = match parent(&walker, &node, trace, request)? {
                Some(value) => value,
                None => return Ok(false),
            };
            if call!(
                trace,
                request,
                "CompareElements(relation.ancestor)",
                self.automation.CompareElements(&node, ancestor)
            )?
            .as_bool()
            {
                trace.record(format_args!("relation_distance={}", depth + 1));
                return Ok(true);
            }
        }
        trace.record(format_args!(
            "relation_walk_end=depth_limit limit={RELATION_DEPTH}"
        ));
        Ok(false)
    }

    /// 即時不一致の原因候補だけを記録し、再 SetFocus・遅延読み取りを行わない。
    unsafe fn diagnose_mismatch(
        &self,
        registered: &IUIAutomationElement,
        initial: &IUIAutomationElement,
        actual: &IUIAutomationElement,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
    ) -> Result<()> {
        trace.stage = "focus.relation";
        let kind = call!(
            trace,
            request,
            "CurrentControlType(actual)",
            actual.CurrentControlType()
        )?;
        let same_before = call!(
            trace,
            request,
            "CompareElements(actual,before)",
            self.automation.CompareElements(actual, initial)
        )?
        .as_bool();
        trace.record(format_args!(
            "actual_control_type={} same_registered=false same_before={same_before}",
            kind.0
        ));
        let relation = if self.ancestor_relation(actual, registered, root, request, trace)? {
            "actual_descendant_of_registered"
        } else if self.ancestor_relation(registered, actual, root, request, trace)? {
            "actual_ancestor_of_registered"
        } else {
            "unknown"
        };
        trace.record(format_args!("relation={relation} diagnostic_only=true"));
        Ok(())
    }

    /// SetFocus 一回の後だけを読み取り、遅延一致を即時成功へ昇格しない。
    unsafe fn observe_late_focus(
        &self,
        registered: &IUIAutomationElement,
        initial: &IUIAutomationElement,
        set_completed: Instant,
        request: Request,
        trace: &mut Trace,
    ) -> Result<()> {
        trace.stage = "focus.late";
        for target_ms in [50u128, 150] {
            loop {
                guard(trace, request)?;
                let Some(wait_ms) = policy::observation_wait(
                    request.started.elapsed().as_millis(),
                    set_completed.elapsed().as_millis(),
                    target_ms,
                ) else {
                    trace.record(format_args!(
                        "target_ms={target_ms} sample=skipped reason=insufficient_request_budget"
                    ));
                    return Err("insufficient_request_budget");
                };
                if wait_ms == 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(wait_ms));
            }
            guard(trace, request)?;
            let sample_start_ms = set_completed.elapsed().as_millis();
            trace.record(format_args!(
                "target_ms={target_ms} sample=start since_set_ms={sample_start_ms}"
            ));
            let actual = call!(
                trace,
                request,
                "GetFocusedElement(late)",
                self.automation.GetFocusedElement()
            )?;
            let same_registered = call!(
                trace,
                request,
                "CompareElements(late,registered)",
                self.automation.CompareElements(&actual, registered)
            )?
            .as_bool();
            let same_before = call!(
                trace,
                request,
                "CompareElements(late,before)",
                self.automation.CompareElements(&actual, initial)
            )?
            .as_bool();
            let kind = call!(
                trace,
                request,
                "CurrentControlType(late)",
                actual.CurrentControlType()
            )?;
            guard(trace, request)?;
            trace.record(format_args!("target_ms={target_ms} sample=end sample_start_ms={sample_start_ms} sample_end_ms={} late_match={same_registered} same_before={same_before} actual_control_type={} diagnostic_only=true", set_completed.elapsed().as_millis(), kind.0));
        }
        Ok(())
    }

    /// F2 登録と F1 一回の移動を従来と同じ条件で実行する。
    unsafe fn perform(&mut self, request: Request, trace: &mut Trace) -> Result<&'static str> {
        trace.stage = "foreground";
        guard(trace, request)?;
        if app::identify(request.window) != Some(request.context.app) {
            return Err("unsupported_or_changed_process");
        }
        trace.stage = "root";
        let root = call!(
            trace,
            request,
            "ElementFromHandle",
            self.automation
                .ElementFromHandle(HWND(request.window.hwnd as *mut _))
        )?;
        trace.stage = "initial_focus";
        let initial_focus = call!(
            trace,
            request,
            "GetFocusedElement(initial)",
            self.automation.GetFocusedElement()
        )?;
        trace.stage = "initial_ancestors";
        let current_document = self.document(&initial_focus, &root, request, trace)?;

        if request.register {
            trace.stage = "registration.replace";
            trace.record(format_args!(
                "old_registration={:?} reason=F2",
                self.registration.as_ref().map(|r| r.id)
            ));
            self.registration = None;
            trace.stage = "registration.edit";
            self.validate_edit(&initial_focus, request, trace)?;
            trace.stage = "registration.verify";
            let actual = call!(
                trace,
                request,
                "GetFocusedElement(register_verify)",
                self.automation.GetFocusedElement()
            )?;
            let same = call!(
                trace,
                request,
                "CompareElements(register_verify)",
                self.automation.CompareElements(&initial_focus, &actual)
            )?
            .as_bool();
            trace.record(format_args!("same_initial={same}"));
            if !same {
                return Err("focus_changed_during_registration");
            }
            guard(trace, request)?;
            self.registration = Some(Registration {
                id: request.context.id,
                window: request.window,
                element: initial_focus,
                document: current_document,
            });
            trace.stage = "registration.created";
            trace.record(format_args!(
                "registration={} reason=F2_verified",
                request.context.id
            ));
            return Ok("REGISTERED");
        }

        trace.stage = "registration.lookup";
        let registered = self.registration.as_ref().ok_or("no_registration")?;
        trace.record(format_args!(
            "registration={} same_window={}",
            registered.id,
            registered.window == request.window
        ));
        if registered.window != request.window {
            return Err("different_window");
        }
        trace.stage = "target.edit";
        self.validate_edit(&registered.element, request, trace)?;
        trace.stage = "target.ancestors";
        let document = self.document(&registered.element, &root, request, trace)?;
        trace.stage = "document.compare";
        let same_target_doc = call!(
            trace,
            request,
            "CompareElements(target_document)",
            self.automation
                .CompareElements(&registered.document, &document)
        )?
        .as_bool();
        trace.record(format_args!("same_target_document={same_target_doc}"));
        if !same_target_doc {
            return Err("different_target_document");
        }
        let same_current_doc = call!(
            trace,
            request,
            "CompareElements(current_document)",
            self.automation
                .CompareElements(&registered.document, &current_document)
        )?
        .as_bool();
        trace.record(format_args!("same_current_document={same_current_doc}"));
        if !same_current_doc {
            return Err("different_current_document");
        }
        trace.stage = "focus.before";
        let actual = call!(
            trace,
            request,
            "GetFocusedElement(before)",
            self.automation.GetFocusedElement()
        )?;
        let same = call!(
            trace,
            request,
            "CompareElements(before,initial)",
            self.automation.CompareElements(&initial_focus, &actual)
        )?
        .as_bool();
        trace.record(format_args!("same_initial={same}"));
        if !same {
            return Err("focus_changed_while_checking");
        }
        let same_registered = call!(
            trace,
            request,
            "CompareElements(before,registered)",
            self.automation
                .CompareElements(&registered.element, &actual)
        )?
        .as_bool();
        trace.record(format_args!("same_registered={same_registered}"));
        trace.stage = "focus.set";
        // 確認と SetFocus は不可分ではない。呼び出しは従来どおり一回のみ。
        guard(trace, request)?;
        let hr = (registered.element.vtable().SetFocus)(registered.element.as_raw());
        let set_completed = Instant::now();
        api(trace, "SetFocus", hr.ok())?;
        trace.record(format_args!(
            "api=SetFocus hr=0x{:08X} result=success",
            hr.0 as u32
        ));
        trace.stage = "focus.after";
        let actual = call!(
            trace,
            request,
            "GetFocusedElement(after)",
            self.automation.GetFocusedElement()
        )?;
        let same = call!(
            trace,
            request,
            "CompareElements(after,registered)",
            self.automation
                .CompareElements(&registered.element, &actual)
        )?
        .as_bool();
        trace.record(format_args!("same_registered={same}"));
        if !same {
            // 即時結果は確定済み。遅延観測中に介入・API失敗があれば追加照会も止める。
            if self
                .observe_late_focus(
                    &registered.element,
                    &initial_focus,
                    set_completed,
                    request,
                    trace,
                )
                .is_err()
            {
                trace.record(format_args!(
                    "late_observation_incomplete=true immediate_match=false"
                ));
                return Err("actual_focus_differs");
            }
            if self
                .diagnose_mismatch(
                    &registered.element,
                    &initial_focus,
                    &actual,
                    &root,
                    request,
                    trace,
                )
                .is_err()
            {
                trace.record(format_args!("relation=unknown diagnostic_incomplete=true"));
            }
            // 追加診断が失敗・期限切れでも元の不一致を成功に変えない。
            trace.stage = "focus.after";
            return Err("actual_focus_differs");
        }
        guard(trace, request)?;
        Ok("FOCUSED")
    }
}
