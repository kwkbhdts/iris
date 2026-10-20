use windows::Win32::{System::Variant::VARIANT, UI::Accessibility::*};

/// 診断と送信で同じ5条件だけを使い、Control View条件は追加しない。
pub fn create(automation: &IUIAutomation2) -> windows::core::Result<IUIAutomationCondition> {
    unsafe {
        let mut condition = automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_EditControlTypeId.0),
        )?;
        for (property, value) in [
            (UIA_IsOffscreenPropertyId, false),
            (UIA_IsEnabledPropertyId, true),
            (UIA_IsKeyboardFocusablePropertyId, true),
            (UIA_IsPasswordPropertyId, false),
        ] {
            let part = automation.CreatePropertyCondition(property, &VARIANT::from(value))?;
            condition = automation.CreateAndCondition(&condition, &part)?;
        }
        Ok(condition)
    }
}
