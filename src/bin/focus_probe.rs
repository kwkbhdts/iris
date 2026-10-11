#[cfg(windows)]
#[path = "../focus/app.rs"]
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
mod policy;
#[cfg(windows)]
#[path = "../focus/uia.rs"]
mod uia;

/// 通常版とは別のコンソールでフォーカスだけを試す。
fn main() {
    #[cfg(windows)]
    app::run();
    #[cfg(not(windows))]
    {
        eprintln!("Iris focus probe は Windows 専用です。");
        std::process::exit(1);
    }
}
