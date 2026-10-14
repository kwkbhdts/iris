use crate::{
    app::{self, Request},
    diagnostic::Trace,
    path_format, policy,
};
use std::sync::atomic::Ordering;
use windows::{
    core::{Interface, BOOL, BSTR},
    Win32::{Foundation::HWND, System::Com::*, UI::Accessibility::*},
};
type Result<T> = std::result::Result<T, &'static str>;

struct Apartment;
impl Drop for Apartment {
    /// 全UIA参照を解放してから専用MTAを終了する。
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

/// 自由文のエラーを出さず、API名とHRESULTで失敗を返す。
fn api<T>(trace: &mut Trace, name: &'static str, value: windows::core::Result<T>) -> Result<T> {
    value.map_err(|error| {
        trace.record(format_args!(
            "api={name} hr=0x{:08X}",
            error.code().0 as u32
        ));
        "uia_error"
    })
}

/// 読み取りの前後でも既存の介入・前面・期限条件を維持する。
fn guard(request: Request, trace: &mut Trace) -> Result<()> {
    app::check(request).map_err(|failure| {
        app::trace_interventions(request, trace, "guard_failed", Some(failure.epoch));
        failure.reason
    })
}
macro_rules! read {
    ($trace:expr, $request:expr, $expr:expr) => {{
        guard($request, $trace)?;
        unsafe { $expr }
    }};
}

/// ClassNameだけを有界表示し、未知と省略を明示する。
fn string_field(
    name: &'static str,
    value: windows::core::Result<BSTR>,
    complete: &mut bool,
) -> String {
    match value {
        Ok(value) => {
            let rendered = path_format::render(&value);
            *complete &= !rendered.truncated && !rendered.invalid_utf16;
            format!(
                "{name}={} {name}_truncated={} {name}_invalid_utf16={}",
                rendered.quoted, rendered.truncated, rendered.invalid_utf16
            )
        }
        Err(error) => {
            *complete = false;
            format!("{name}=unknown {name}_hr=0x{:08X}", error.code().0 as u32)
        }
    }
}

/// ビュー所属の取得失敗をfalseとして扱わない。
fn bool_field(
    name: &'static str,
    value: windows::core::Result<BOOL>,
    complete: &mut bool,
) -> String {
    match value {
        Ok(value) => format!("{name}={}", value.as_bool()),
        Err(error) => {
            *complete = false;
            format!("{name}=unknown {name}_hr=0x{:08X}", error.code().0 as u32)
        }
    }
}

impl Session {
    /// 専用MTAへUIAを作り、既存と同じ200msの応答設定を使う。
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

    /// 診断を要求内に留め、失敗時も部分観測であることを表示する。
    pub fn execute(&mut self, request: Request) -> String {
        let mut trace = Trace::new(request.context);
        let result = self.inspect(request, &mut trace);
        if let Err(reason) = result {
            trace.record(format_args!(
                "reason={reason} complete=false no_focus_write=true"
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

    /// 先に前面ルートへの所属を確かめ、その一本だけの属性を取得する。
    fn inspect(&self, request: Request, trace: &mut Trace) -> Result<()> {
        if !crate::TARGET_SCOPE.allows(request.context.app) {
            return Err("unsupported_app");
        }
        trace.stage = "reference";
        let root_result = read!(
            trace,
            request,
            self.automation
                .ElementFromHandle(HWND(request.window.hwnd as *mut _))
        );
        let root = api(trace, "ElementFromHandle", root_result)?;
        let reference_result = read!(trace, request, self.automation.GetFocusedElement());
        let reference = api(trace, "GetFocusedElement", reference_result)?;
        let type_result = read!(trace, request, reference.CurrentControlType());
        let kind = api(trace, "ControlType", type_result)?;
        trace.record(format_args!(
            "control_type={} expected_edit={} empty_input_is_user_precondition=true",
            kind.0,
            kind == UIA_EditControlTypeId
        ));
        if kind != UIA_EditControlTypeId {
            return Err("reference_not_edit");
        }
        let walker_result = read!(trace, request, self.automation.RawViewWalker());
        let walker = api(trace, "RawViewWalker", walker_result)?;
        let mut path = Vec::new();
        let mut current = reference.clone();
        let mut root_reached = false;
        trace.stage = "path.scope";
        for _ in 0..path_format::PATH_LIMIT {
            let same_result = read!(
                trace,
                request,
                self.automation.CompareElements(&current, &root)
            );
            let same = api(trace, "CompareElements(root)", same_result)?.as_bool();
            path.push(current.clone());
            if same {
                root_reached = true;
                break;
            }
            // 64番目の外にある親は取得せず、範囲外の文字列も読まない。
            if path.len() == path_format::PATH_LIMIT {
                break;
            }
            guard(request, trace)?;
            let mut raw = std::ptr::null_mut();
            let hr = unsafe {
                (walker.vtable().GetParentElement)(walker.as_raw(), current.as_raw(), &mut raw)
            };
            let parent = if raw.is_null() {
                None
            } else {
                Some(unsafe { IUIAutomationElement::from_raw(raw) })
            };
            api(trace, "GetParentElement", hr.ok())?;
            current = parent.ok_or("no_parent_before_root")?;
        }
        trace.record(format_args!(
            "nodes={} root_reached={root_reached} max_nodes={}",
            path.len(),
            path_format::PATH_LIMIT
        ));
        if !root_reached {
            return Err("path_limit");
        }
        let mut complete = true;
        trace.stage = "path.node";
        for (depth, node) in path.iter().enumerate() {
            let kind_result = read!(trace, request, node.CurrentControlType());
            let kind = api(trace, "ControlType", kind_result)?;
            let class = string_field(
                "class_name",
                read!(trace, request, node.CurrentClassName()),
                &mut complete,
            );
            // IDは有無への変換後に解放し、生値を表示用バッファへ渡さない。
            let id_result = read!(trace, request, node.CurrentAutomationId());
            let id = match id_result {
                Ok(value) => path_format::presence(Some(&value)),
                Err(error) => {
                    complete = false;
                    trace.record(format_args!(
                        "api=AutomationId hr=0x{:08X}",
                        error.code().0 as u32
                    ));
                    path_format::presence(None)
                }
            };
            let control = bool_field(
                "is_control_element",
                read!(trace, request, node.CurrentIsControlElement()),
                &mut complete,
            );
            guard(request, trace)?;
            trace.record(format_args!("depth={depth} control_type={} root_match={} {class} automation_id_present={id} {control}", kind.0, depth + 1 == path.len()));
        }
        let actual_result = read!(trace, request, self.automation.GetFocusedElement());
        let actual = api(trace, "GetFocusedElement(final)", actual_result)?;
        let same_result = read!(
            trace,
            request,
            self.automation.CompareElements(&reference, &actual)
        );
        if !api(trace, "CompareElements(final)", same_result)?.as_bool() {
            return Err("reference_focus_changed");
        }
        guard(request, trace)?;
        trace.stage = "path.summary";
        trace.record(format_args!("path_complete=true nodes={} metadata_complete={complete} other_branches_read=false chat_input_identified=false", path.len()));
        if complete {
            Ok(())
        } else {
            Err("metadata_unknown_or_truncated")
        }
    }
}
