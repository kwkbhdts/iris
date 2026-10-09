#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(any(windows, test))]
#[path = "../diagnostic/model.rs"]
mod model;
#[cfg(windows)]
#[path = "../diagnostic/windows.rs"]
mod windows;

/// 入力操作を行わない診断ウィンドウを開く。
#[cfg(windows)]
fn main() {
    windows::run();
}

/// 未対応環境では診断を開始しない。
#[cfg(not(windows))]
fn main() {
    eprintln!("Iris 診断版は Windows 専用です。");
    std::process::exit(1);
}
