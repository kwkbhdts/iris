use super::*;
use crate::{
    ancestry::Tree,
    failure::{self, Identity, Probe},
};

struct ReadOnly<'a> {
    session: &'a Session,
    target: &'a Target,
    root: &'a IUIAutomationElement,
    request: Request,
    permitted: Option<[u64; 2]>,
    saved: (types::Snapshot, types::WindowId, usize),
    trace: &'a mut Trace,
}
impl ReadOnly<'_> {
    /// 復元失敗ラッチには触れず、元の二つの期限と介入条件を確認する。
    fn check(&self) -> Result<()> {
        app::check_send(self.request, self.permitted)?;
        if !model::within_deadline(self.saved.0.at, unsafe {
            windows_sys::Win32::System::SystemInformation::GetTickCount()
        }) {
            return Err("diagnostic_ime_deadline");
        }
        Ok(())
    }
}
impl Probe for ReadOnly<'_> {
    type Node = IUIAutomationElement;

    /// 診断の前後でも同じ要求だけを観測する。
    fn guard(&mut self) -> Result<()> {
        self.check()
    }

    /// 保存入力先と現在値を比較し、生の識別情報をログへ出さない。
    fn identity(&mut self) -> [Option<bool>; 5] {
        let (foreground, focus, ime, layout) = ime::read_identity();
        failure::compare(
            Identity {
                foreground: self.saved.0.foreground.ok(),
                focus: self.saved.0.focus.ok(),
                ime: Some(self.saved.1),
                layout: (self.saved.2 != 0).then_some(self.saved.2),
            },
            Identity {
                foreground,
                focus,
                ime,
                layout,
            },
        )
    }

    /// 現在フォーカスを新規取得し、エラーを別要素と扱わない。
    fn focus(&mut self) -> Result<Self::Node> {
        self.check()?;
        api(self.trace, "GetFocusedElement(failure)", unsafe {
            self.session.automation.GetFocusedElement()
        })
    }

    /// 保存要素と現在フォーカスの一致だけを比較する。
    fn same(&mut self, node: &Self::Node) -> Result<bool> {
        self.check()?;
        api(self.trace, "CompareElements(failure_focus)", unsafe {
            self.session
                .automation
                .CompareElements(&self.target.element, node)
        })
        .map(|v| v.as_bool())
    }

    /// 新しいフォーカスのルート到達だけを読み、候補として採用しない。
    fn path(&mut self, node: Self::Node) -> ancestry::Observation {
        ancestry::inspect(self, node)
    }
}
impl Tree for ReadOnly<'_> {
    type Node = IUIAutomationElement;

    /// 親方向の各区間も期限内の読取りだけにする。
    fn check(&mut self) -> Result<()> {
        ReadOnly::check(self)
    }

    /// 保存ルートのUIA同一性を確認する。
    fn is_root(&mut self, node: &Self::Node) -> Result<bool> {
        self.check()?;
        api(self.trace, "CompareElements(failure_root)", unsafe {
            self.session.automation.CompareElements(self.root, node)
        })
        .map(|v| v.as_bool())
    }

    /// 追加診断はルート所属だけを調べ、内容・属性を増やさない。
    fn is_document(&mut self, _: &Self::Node) -> Result<bool> {
        Ok(false)
    }

    /// NULLと失敗を区別し、失敗後に別の探索へ乗り換えない。
    fn parent(&mut self, node: &Self::Node) -> Result<Option<Self::Node>> {
        self.check()?;
        let mut raw = std::ptr::null_mut();
        let hr = unsafe {
            (self.session.walker.vtable().GetParentElement)(
                self.session.walker.as_raw(),
                node.as_raw(),
                &mut raw,
            )
        };
        let parent = if raw.is_null() {
            None
        } else {
            Some(unsafe { IUIAutomationElement::from_raw(raw) })
        };
        api(self.trace, "GetParentElement(failure)", hr.ok())?;
        self.check()?;
        Ok(parent)
    }
}
impl Session {
    /// 復元拒否を維持したまま一回だけ観測する。診断結果で復元・再送しない。
    pub(super) fn diagnose_restore_failure(
        &self,
        target: &Target,
        root: &IUIAutomationElement,
        request: Request,
        permitted: Option<[u64; 2]>,
        saved: (types::Snapshot, types::WindowId, usize),
        trace: &mut Trace,
    ) {
        trace.stage = "restore.failure_observation";
        let sample = failure::observe(&mut ReadOnly {
            session: self,
            target,
            root,
            request,
            permitted,
            saved,
            trace,
        });
        trace.record(format_args!("sample=1 read_only=true restore_remains_rejected=true fresh_focus_same_saved={} fresh_focus_root_reached={} fresh_focus_ancestry_complete={} fresh_focus_parent_missing_depth={:?} path_reason={} foreground_same={} win32_focus_same={} ime_window_same={} ime_owner_same={} layout_same={} observation_complete={} reason={}", failure::label(sample.same), failure::label(if sample.path.complete { Some(sample.path.root_reached) } else { None }), sample.path.complete, sample.path.parent_missing_depth, sample.path.reason, failure::label(sample.identity[0]), failure::label(sample.identity[1]), failure::label(sample.identity[2]), failure::label(sample.identity[3]), failure::label(sample.identity[4]), sample.complete, sample.reason));
    }
}
