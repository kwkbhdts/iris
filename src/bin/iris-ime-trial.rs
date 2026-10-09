#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(any(windows, test))]
#[allow(dead_code)]
#[path = "../diagnostic/model.rs"]
mod diagnostic;
#[cfg(any(windows, test))]
#[path = "../ime_trial/model.rs"]
mod model;
#[cfg(windows)]
#[path = "../ime_trial/windows.rs"]
mod windows;

/// 入力送信を含まない IME 切替試験を開く。
#[cfg(windows)]
fn main() {
    windows::run();
}

/// 未対応環境では試験を開始しない。
#[cfg(not(windows))]
fn main() {
    eprintln!("Iris IME 切替試験版は Windows 専用です。");
    std::process::exit(1);
}
