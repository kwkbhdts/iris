use crate::{
    app::{self, Request},
    diagnostic::{App, Trace},
    policy,
    scan::{self, Tree},
};
use scan::Result;
use std::sync::atomic::Ordering;
use windows::{
    core::{IUnknown, Interface, BSTR},
    Win32::{Foundation::HWND, System::Com::*, UI::Accessibility::*},
};

struct Apartment;
impl Drop for Apartment {
    /// UIA参照を破棄してから専用MTAを終了する。
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
pub struct Session {
    automation: IUIAutomation2,
    _apartment: Apartment,
}

/// 本文由来のエラー説明を出さず、APIとHRESULTだけを記録する。
fn api<T>(trace: &mut Trace, name: &'static str, value: windows::core::Result<T>) -> Result<T> {
    value.map_err(|error| {
        trace.record(format_args!(
            "api={name} hr=0x{:08X}",
            error.code().0 as u32
        ));
        "uia_error"
    })
}
/// 介入・前面・期限が変わったら読み取りも中止する。
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

/// S_OKのNULLを「子・兄弟・親なし」とし、他の失敗と区別する。
fn navigate(
    walker: &IUIAutomationTreeWalker,
    node: &IUIAutomationElement,
    direction: u8,
    request: Request,
    trace: &mut Trace,
) -> Result<Option<IUIAutomationElement>> {
    guard(request, trace)?;
    let mut raw = std::ptr::null_mut();
    let hr = unsafe {
        let table = walker.vtable();
        match direction {
            0 => (table.GetFirstChildElement)(walker.as_raw(), node.as_raw(), &mut raw),
            1 => (table.GetNextSiblingElement)(walker.as_raw(), node.as_raw(), &mut raw),
            _ => (table.GetParentElement)(walker.as_raw(), node.as_raw(), &mut raw),
        }
    };
    let element = if raw.is_null() {
        None
    } else {
        Some(unsafe { IUIAutomationElement::from_raw(raw) })
    };
    api(trace, "TreeWalker", hr.ok())?;
    Ok(element)
}

/// パターンの存在だけを照会し、ValueやTextの内容は読まない。
fn pattern(
    node: &IUIAutomationElement,
    id: UIA_PATTERN_ID,
    request: Request,
    trace: &mut Trace,
) -> Result<Option<bool>> {
    guard(request, trace)?;
    let mut raw = std::ptr::null_mut();
    let hr = unsafe { (node.vtable().GetCurrentPattern)(node.as_raw(), id, &mut raw) };
    let present = !raw.is_null();
    if present {
        drop(unsafe { IUnknown::from_raw(raw) });
    }
    if hr.is_ok() {
        Ok(Some(present))
    } else if hr.0 as u32 == UIA_E_NOTSUPPORTED {
        Ok(Some(false))
    } else {
        trace.record(format_args!(
            "pattern_id={} hr=0x{:08X} available=unknown",
            id.0, hr.0 as u32
        ));
        Ok(None)
    }
}

struct Identity {
    id: Option<BSTR>,
    class: Option<BSTR>,
}
/// 識別文字列は要求中だけ保持し、空か・同値か以外を出力しない。
fn identity(node: &IUIAutomationElement, request: Request, trace: &mut Trace) -> Result<Identity> {
    guard(request, trace)?;
    let id = api(trace, "CurrentAutomationId", unsafe {
        node.CurrentAutomationId()
    })
    .ok();
    guard(request, trace)?;
    let class = api(trace, "CurrentClassName", unsafe {
        node.CurrentClassName()
    })
    .ok();
    Ok(Identity { id, class })
}
/// 空文字列同士を識別根拠として数えない。
fn same_nonempty(a: &Option<BSTR>, b: &Option<BSTR>) -> Option<bool> {
    match (a, b) {
        (Some(a), Some(b)) => Some(!a.is_empty() && !b.is_empty() && a == b),
        _ => None,
    }
}
/// 自由文を取得せず、編集条件とパターン対応をまとめる。
fn attributes(node: &IUIAutomationElement, request: Request, trace: &mut Trace) -> Result<bool> {
    let enabled = read!(trace, request, "IsEnabled", node.CurrentIsEnabled())?.as_bool();
    let focusable = read!(
        trace,
        request,
        "IsKeyboardFocusable",
        node.CurrentIsKeyboardFocusable()
    )?
    .as_bool();
    let offscreen = read!(trace, request, "IsOffscreen", node.CurrentIsOffscreen())?.as_bool();
    let password = read!(trace, request, "IsPassword", node.CurrentIsPassword())?.as_bool();
    let value = pattern(node, UIA_ValuePatternId, request, trace)?;
    let text = pattern(node, UIA_TextPatternId, request, trace)?;
    let text2 = pattern(node, UIA_TextPattern2Id, request, trace)?;
    trace.record(format_args!("enabled={enabled} focusable={focusable} offscreen={offscreen} password={password} value_pattern={value:?} text_pattern={text:?} text2_pattern={text2:?}"));
    Ok(policy::editable(
        true, enabled, focusable, offscreen, password,
    ))
}

struct Survey<'a> {
    walker: IUIAutomationTreeWalker,
    automation: &'a IUIAutomation2,
    reference: IUIAutomationElement,
    identity: Identity,
    reference_is_edit: bool,
    request: Request,
    trace: &'a mut Trace,
    visited: usize,
    edits: usize,
    candidates: usize,
    reference_matches: usize,
    id_matches: usize,
    class_matches: usize,
    identity_unknown: usize,
}
impl Tree for Survey<'_> {
    type Node = IUIAutomationElement;
    /// Raw Viewの全子を対象とし、先頭候補で止めない。
    fn child(&mut self, node: &Self::Node) -> Result<Option<Self::Node>> {
        navigate(&self.walker, node, 0, self.request, self.trace)
    }
    /// 兄弟取得失敗を探索完了へ変換しない。
    fn sibling(&mut self, node: &Self::Node) -> Result<Option<Self::Node>> {
        navigate(&self.walker, node, 1, self.request, self.trace)
    }
    /// 全Editの位置と条件を記録し、条件に合う候補と正解との一致を数える。
    fn visit(
        &mut self,
        node: &Self::Node,
        index: usize,
        parent: Option<usize>,
        depth: usize,
        in_document: bool,
    ) -> Result<bool> {
        self.visited += 1;
        let trace = &mut *self.trace;
        let request = self.request;
        let kind = read!(trace, request, "ControlType", node.CurrentControlType())?;
        if kind == UIA_EditControlTypeId {
            if self.edits >= scan::EDIT_LIMIT {
                return Err("edit_limit");
            }
            self.edits += 1;
            trace.stage = "edit";
            let same = read!(
                trace,
                request,
                "CompareElements",
                self.automation.CompareElements(node, &self.reference)
            )?
            .as_bool();
            trace.record(format_args!("edit={} node={index} parent={parent:?} depth={depth} in_document={in_document} control_type={} same_reference={same}", self.edits, kind.0));
            let eligible = attributes(node, request, trace)? && in_document;
            let identity = identity(node, request, trace)?;
            let id_same = same_nonempty(&identity.id, &self.identity.id);
            let class_same = same_nonempty(&identity.class, &self.identity.class);
            trace.record(format_args!("candidate={eligible} automation_id_present={:?} class_name_present={:?} id_same_reference={id_same:?} class_same_reference={class_same:?}", identity.id.as_ref().map(|v| !v.is_empty()), identity.class.as_ref().map(|v| !v.is_empty())));
            if eligible {
                self.candidates += 1;
                self.reference_matches += usize::from(same);
                self.id_matches += usize::from(id_same == Some(true));
                self.class_matches += usize::from(class_same == Some(true));
                self.identity_unknown += usize::from(id_same.is_none() || class_same.is_none());
            }
        }
        // 前面ルート自身のDocumentを文書所属の証明に使わない。
        Ok(in_document || (depth > 0 && kind == UIA_DocumentControlTypeId))
    }
}

impl Session {
    /// UIAだけを専用MTAへ作成し、既存と同じ200ms制限を使う。
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
            Ok(Self {
                automation,
                _apartment: apartment,
            })
        }
    }
    /// 結果は観測完了か途中終了だけで、フォーカス成功とは表示しない。
    pub fn execute(&mut self, request: Request) -> String {
        let mut trace = Trace::new(request.context);
        let result = self.inspect(request, &mut trace);
        if let Err(reason) = result {
            trace.record(format_args!(
                "reason={reason} complete=false unique_candidate=false no_focus_write=true"
            ));
        }
        if request.started.elapsed().as_millis() >= policy::BUDGET_MS {
            app::STOP.store(true, Ordering::SeqCst);
        }
        trace.finish(
            if result.is_ok() {
                "OBSERVED"
            } else {
                "INCOMPLETE"
            },
            app::STOP.load(Ordering::SeqCst),
        )
    }
    /// 観測したフォーカスの祖先とルートを調べ、非Editも正解扱いせず列挙へ進む。
    fn inspect(&self, request: Request, trace: &mut Trace) -> Result<()> {
        if request.context.app != App::ChatGpt {
            return Err("unsupported_app");
        }
        trace.stage = "reference";
        let root = read!(
            trace,
            request,
            "ElementFromHandle",
            self.automation
                .ElementFromHandle(HWND(request.window.hwnd as *mut _))
        )?;
        let reference = read!(
            trace,
            request,
            "GetFocusedElement",
            self.automation.GetFocusedElement()
        )?;
        let kind = read!(
            trace,
            request,
            "ControlType",
            reference.CurrentControlType()
        )?;
        trace.record(format_args!(
            "control_type={} expected_edit={}",
            kind.0,
            kind == UIA_EditControlTypeId
        ));
        let reference_is_edit = kind == UIA_EditControlTypeId;
        trace.record(format_args!(
            "reference_role=observed_focus reference_is_edit={reference_is_edit} reference_identity_applicable={reference_is_edit} chat_input_identified=false"
        ));
        // 非Editには編集属性や識別文字列を要求せず、型と祖先だけを記録する。
        let reference_identity = if reference_is_edit {
            if !attributes(&reference, request, trace)? {
                return Err("reference_not_eligible");
            }
            let value = identity(&reference, request, trace)?;
            trace.record(format_args!(
                "automation_id_present={:?} class_name_present={:?} raw_strings_omitted=true",
                value.id.as_ref().map(|v| !v.is_empty()),
                value.class.as_ref().map(|v| !v.is_empty())
            ));
            value
        } else {
            trace.record(format_args!("reference_details=skipped_non_edit"));
            Identity {
                id: None,
                class: None,
            }
        };
        let walker = read!(
            trace,
            request,
            "RawViewWalker",
            self.automation.RawViewWalker()
        )?;
        let mut node = reference.clone();
        let mut reached = false;
        trace.stage = "reference.ancestors";
        for depth in 0..scan::DEPTH_LIMIT {
            let same = read!(
                trace,
                request,
                "CompareElements(root)",
                self.automation.CompareElements(&node, &root)
            )?
            .as_bool();
            let kind = read!(trace, request, "ControlType", node.CurrentControlType())?;
            trace.record(format_args!(
                "depth={depth} control_type={} root_match={same}",
                kind.0
            ));
            if same {
                reached = true;
                break;
            }
            node =
                navigate(&walker, &node, 2, request, trace)?.ok_or("reference_root_not_reached")?;
        }
        if !reached {
            return Err("reference_depth_limit");
        }
        let mut survey = Survey {
            walker,
            automation: &self.automation,
            reference: reference.clone(),
            identity: reference_identity,
            reference_is_edit,
            request,
            trace,
            visited: 0,
            edits: 0,
            candidates: 0,
            reference_matches: 0,
            id_matches: 0,
            class_matches: 0,
            identity_unknown: 0,
        };
        let result =
            scan::walk(&mut survey, root, scan::NODE_LIMIT, scan::DEPTH_LIMIT).and_then(|_| {
                let actual = read!(
                    survey.trace,
                    request,
                    "GetFocusedElement(final)",
                    self.automation.GetFocusedElement()
                )?;
                let same = read!(
                    survey.trace,
                    request,
                    "CompareElements(final)",
                    self.automation.CompareElements(&reference, &actual)
                )?
                .as_bool();
                if !same {
                    return Err("reference_focus_changed");
                }
                guard(request, survey.trace)
            });
        survey.trace.stage = "survey";
        survey.trace.record(format_args!("complete={} reason={} visited={} edits={} candidates={} reference_matches={} id_matches={} class_matches={} identity_unknown={} reference_identity_applicable={} unique_candidate={} chat_input_identified=false", result.is_ok(), result.as_ref().err().copied().unwrap_or("exhausted"), survey.visited, survey.edits, survey.candidates, survey.reference_matches, survey.id_matches, survey.class_matches, survey.identity_unknown, survey.reference_is_edit, scan::unique(result.is_ok(), survey.reference_is_edit, survey.candidates, survey.reference_matches)));
        result.map(|_| ())
    }
}
