use super::*;
use crate::root_search::{self, Probe};

struct Search<'a> {
    session: &'a Session,
    root: &'a IUIAutomationElement,
    request: Request,
    trace: &'a mut Trace,
    outcome: &'a mut Outcome,
}
impl Probe for Search<'_> {
    type Target = Target;

    /// 毎回同じ前面ルートと明示的な阻害要素を確認する。
    fn check_root(&mut self) -> Result<()> {
        self.session
            .unblocked(self.root, self.request, self.trace, self.outcome)
    }

    /// 候補自身のDocument・祖先順序・ルート所属を既存検索で確認する。
    fn find_target(&mut self) -> Result<Target> {
        self.session
            .search(self.root, self.request, self.trace, self.outcome)
    }
}

impl Session {
    /// 初期フォーカスの位置によらず、前面ルート全体を同じ条件で探索する。
    pub(super) fn search_root(
        &self,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<Target> {
        trace.record(format_args!(
            "scope=foreground_root initial_document_required=false"
        ));
        root_search::run(&mut Search {
            session: self,
            root,
            request,
            trace,
            outcome,
        })
    }

    /// 明示された画面内ダイアログは、候補の兄弟領域でも拒否する。
    pub(super) fn unblocked(
        &self,
        root: &IUIAutomationElement,
        request: Request,
        trace: &mut Trace,
        outcome: &mut Outcome,
    ) -> Result<()> {
        guard(request, trace, outcome)?;
        // 子HWNDやownerへ探索範囲を勝手に広げず、前面の最上位だけを許可する。
        let hwnd = request.window.hwnd as windows_sys::Win32::Foundation::HWND;
        let top = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::GetAncestor(
                hwnd,
                windows_sys::Win32::UI::WindowsAndMessaging::GA_ROOT,
            )
        };
        if top != hwnd {
            return Err("foreground_not_top_level");
        }
        let current = call!(
            trace,
            request,
            outcome,
            "ElementFromHandle(root_check)",
            self.automation
                .ElementFromHandle(HWND(request.window.hwnd as *mut _))
        )?;
        if !call!(
            trace,
            request,
            outcome,
            "CompareElements(root_check)",
            self.automation.CompareElements(root, &current)
        )?
        .as_bool()
        {
            return Err("root_changed");
        }
        if !call!(
            trace,
            request,
            outcome,
            "IsEnabled(root)",
            root.CurrentIsEnabled()
        )?
        .as_bool()
        {
            return Err("root_disabled");
        }
        let list = call!(
            trace,
            request,
            outcome,
            "FindAll(dialog_or_modal)",
            root.FindAll(TreeScope_Subtree, &self.blocker_condition)
        )?;
        let count = call!(
            trace,
            request,
            outcome,
            "Length(dialog_or_modal)",
            list.Length()
        )?;
        guard(request, trace, outcome)?;
        trace.record(format_args!(
            "visible_dialog_or_modal_count={count} unreported_overlay_state=unknown"
        ));
        match count {
            0 => Ok(()),
            1.. => Err("visible_dialog_or_modal"),
            _ => Err("invalid_blocker_count"),
        }
    }
}
