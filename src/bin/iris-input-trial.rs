#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(any(windows, test))]
#[allow(dead_code)]
#[path = "../diagnostic/model.rs"]
mod diagnostic;
#[cfg(any(windows, test))]
#[path = "../input_trial/model.rs"]
mod model;
#[cfg(windows)]
#[path = "../input_trial/windows.rs"]
mod windows;

/// IME 対応の入力比較ウィンドウを開く。
#[cfg(windows)]
fn main() {
    windows::run();
}

/// 未対応環境では試験を開始しない。
#[cfg(not(windows))]
fn main() {
    eprintln!("Iris IME 入力比較版は Windows 専用です。");
    std::process::exit(1);
}
