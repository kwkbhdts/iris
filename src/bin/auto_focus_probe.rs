#[cfg(any(windows, test))]
const TARGET_SCOPE: diagnostic::TargetScope = diagnostic::TargetScope::ChatGptOnly;

#[cfg(windows)]
#[path = "../build_info.rs"]
mod build_info;
#[cfg(windows)]
const AUTO_SEND: bool = false;
#[cfg(windows)]
const OWN_INPUT: Option<fn(bool, usize) -> bool> = None;
#[cfg(windows)]
const AUTO_FOCUS: bool = true;
#[cfg(windows)]
const PATH_ONLY: bool = false;
#[cfg(windows)]
const DISCOVERY_ONLY: bool = true;
#[cfg(windows)]
#[path = "../focus/app.rs"]
#[allow(dead_code)]
mod app;
#[cfg(any(windows, test))]
#[path = "../focus/diagnostic.rs"]
mod diagnostic;
#[cfg(any(windows, test))]
#[path = "../focus/intervention.rs"]
mod intervention;
#[cfg(any(windows, test))]
#[path = "../logic.rs"]
#[allow(dead_code)]
mod logic;
#[cfg(any(windows, test))]
#[path = "../focus/policy.rs"]
#[allow(dead_code)]
mod policy;
#[cfg(any(windows, test))]
#[path = "../auto_focus/post_cancel.rs"]
mod post_cancel;
#[cfg(any(windows, test))]
#[path = "../auto_focus/rules.rs"]
#[cfg_attr(not(windows), allow(dead_code))]
mod rules;
#[cfg(windows)]
#[path = "../auto_focus/uia.rs"]
mod uia;

/// 通常版とは別のコンソールで登録なしでフォーカス移動だけを試す。
fn main() {
    #[cfg(windows)]
    app::run();
    #[cfg(not(windows))]
    {
        eprintln!("Iris auto-focus probe は Windows 専用です。");
        std::process::exit(1);
    }
}
