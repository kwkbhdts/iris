use crate::{
    app::{self, Request, Window},
    policy,
};
use windows::Win32::{Foundation::HWND, System::Com::*, UI::Accessibility::*};

type Result<T> = std::result::Result<T, &'static str>;
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
    window: Window,
    element: IUIAutomationElement,
    document: IUIAutomationElement,
}
pub struct Session {
    automation: IUIAutomation2,
    registration: Option<Registration>,
    _apartment: Apartment,
}

/// HRESULT の詳細やプロバイダー文字列をログへ漏らさず失敗扱いにする。
fn api<T>(result: windows::core::Result<T>) -> Result<T> {
    result.map_err(|_| "UIA call failed / element unavailable")
}

impl Session {
    /// タイムアウトを持つ UIA を同じ専用スレッド内に作る。
    pub fn new() -> Result<Self> {
        unsafe {
            api(CoInitializeEx(None, COINIT_MULTITHREADED).ok())?;
            let apartment = Apartment;
            let automation: IUIAutomation2 = api(CoCreateInstance(
                &CUIAutomation8,
                None,
                CLSCTX_INPROC_SERVER,
            ))?;
            api(automation.SetConnectionTimeout(200))?;
            api(automation.SetTransactionTimeout(200))?;
            Ok(Self {
                automation,
                registration: None,
                _apartment: apartment,
            })
        }
    }

    /// 失敗した登録を次回へ持ち越さず、勝手な再探索を行わない。
    pub fn execute(&mut self, request: Request) -> Result<&'static str> {
        let result = unsafe { self.perform(request) };
        if result.is_err() {
            self.registration = None;
        }
        result
    }

    /// 数値の型と可操作性だけを読み、名前・値・本文は取得しない。
    unsafe fn validate_edit(&self, element: &IUIAutomationElement, request: Request) -> Result<()> {
        app::check(request)?;
        let kind = api(element.CurrentControlType())?;
        println!("control_type={}", kind.0);
        let enabled = api(element.CurrentIsEnabled())?.as_bool();
        app::check(request)?;
        let focusable = api(element.CurrentIsKeyboardFocusable())?.as_bool();
        let offscreen = api(element.CurrentIsOffscreen())?.as_bool();
        app::check(request)?;
        let password = api(element.CurrentIsPassword())?.as_bool();
        if !policy::editable(
            kind == UIA_EditControlTypeId,
            enabled,
            focusable,
            offscreen,
            password,
        ) {
            return Err("requires a visible, enabled, focusable, non-password UIA Edit");
        }
        app::check(request)?;
        Ok(())
    }

    /// 有限の親探索で前面ルートへの所属と一番近い文書を確認する。
    unsafe fn document(
        &self,
        element: &IUIAutomationElement,
        root: &IUIAutomationElement,
        request: Request,
    ) -> Result<IUIAutomationElement> {
        let walker = api(self.automation.RawViewWalker())?;
        let mut node = element.clone();
        let mut document = None;
        for _ in 0..policy::MAX_ANCESTORS {
            app::check(request)?;
            if api(self.automation.CompareElements(&node, root))?.as_bool() {
                return document
                    .ok_or("no document ancestor; browser chrome and unknown layouts rejected");
            }
            if document.is_none() && api(node.CurrentControlType())? == UIA_DocumentControlTypeId {
                document = Some(node.clone());
            }
            app::check(request)?;
            node = api(walker.GetParentElement(&node))?;
        }
        Err("ancestor limit reached / not in foreground window")
    }

    /// F2 の明示登録か、同じ文書内の登録済み一要素への F1 移動だけを行う。
    unsafe fn perform(&mut self, request: Request) -> Result<&'static str> {
        app::check(request)?;
        if !app::allowed(request.window) {
            return Err("unsupported foreground process");
        }
        let root = api(self
            .automation
            .ElementFromHandle(HWND(request.window.hwnd as *mut _)))?;
        let initial_focus = api(self.automation.GetFocusedElement())?;
        let current_document = self.document(&initial_focus, &root, request)?;

        if request.register {
            self.registration = None;
            self.validate_edit(&initial_focus, request)?;
            let actual = api(self.automation.GetFocusedElement())?;
            if !api(self.automation.CompareElements(&initial_focus, &actual))?.as_bool() {
                return Err("focus changed during registration");
            }
            app::check(request)?;
            self.registration = Some(Registration {
                window: request.window,
                element: initial_focus,
                document: current_document,
            });
            return Ok(
                "REGISTERED: one explicit Edit in this window/document (F2 does not move focus)",
            );
        }

        let registered = self
            .registration
            .as_ref()
            .ok_or("no registration; click the composer and press F2 first")?;
        if registered.window != request.window {
            return Err("different window; register again");
        }
        self.validate_edit(&registered.element, request)?;
        let document = self.document(&registered.element, &root, request)?;
        if !api(self
            .automation
            .CompareElements(&registered.document, &document))?
        .as_bool()
            || !api(self
                .automation
                .CompareElements(&registered.document, &current_document))?
            .as_bool()
        {
            return Err("different document/tab; register again");
        }
        let actual = api(self.automation.GetFocusedElement())?;
        if !api(self.automation.CompareElements(&initial_focus, &actual))?.as_bool() {
            return Err("focus changed while checking");
        }
        app::check(request)?;
        // SetFocus 自体と直前の確認は不可分にはできない。再試行や代替入力はしない。
        api(registered.element.SetFocus())?;
        app::check(request)?;
        let actual = api(self.automation.GetFocusedElement())?;
        if !api(self
            .automation
            .CompareElements(&registered.element, &actual))?
        .as_bool()
        {
            return Err("SetFocus returned but actual focus differs");
        }
        app::check(request)?;
        Ok("FOCUSED: confirmed registered element; no text/Enter/IME writes")
    }
}
