#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod ime;
#[cfg(any(windows, test))]
mod logic;
#[cfg(any(windows, test))]
mod model;
#[cfg(any(windows, test))]
#[allow(dead_code)]
mod types;
#[cfg(windows)]
mod windows;

/// Windows の通知領域アプリを開始する。
#[cfg(windows)]
fn main() {
    windows::run();
}

/// 未対応の環境では常駐せず終了する。
#[cfg(not(windows))]
fn main() {
    eprintln!("Iris は Windows 専用です。");
    std::process::exit(1);
}
