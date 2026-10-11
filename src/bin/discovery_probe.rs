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
#[path = "../discovery/scan.rs"]
#[cfg_attr(not(windows), allow(dead_code))]
mod scan;
#[cfg(windows)]
#[path = "../discovery/uia.rs"]
mod uia;

/// 通常版とは別のコンソールで読み取りだけを試す。
fn main() {
    #[cfg(windows)]
    app::run();
    #[cfg(not(windows))]
    {
        eprintln!("Iris discovery probe は Windows 専用です。");
        std::process::exit(1);
    }
}
