#[cfg(windows)]
static INSPECT_ONLY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

/// 起動時だけ選んだ読み取りモードを、要求中には変更しない。
#[cfg(windows)]
fn inspect_only() -> bool {
    *INSPECT_ONLY.get().expect("mode initialized before hooks")
}

#[cfg(any(windows, test))]
#[path = "../auto_send/mode.rs"]
mod mode;

#[cfg(any(windows, test))]
const TARGET_SCOPE: diagnostic::TargetScope = diagnostic::TargetScope::AutoSend;

#[cfg(windows)]
#[path = "../build_info.rs"]
mod build_info;
#[cfg(windows)]
#[path = "../warning.rs"]
#[allow(dead_code)]
mod warning;
#[cfg(windows)]
const AUTO_SEND: bool = true;
#[cfg(windows)]
const OWN_INPUT: Option<fn(bool, usize) -> bool> = Some(is_own_input);

/// 既存の注入タグ判定だけを共有フックへ渡す。
#[cfg(windows)]
fn is_own_input(injected: bool, tag: usize) -> bool {
    model::own_input(injected, tag, ime::input_tag())
}
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
#[cfg(windows)]
#[path = "../ime.rs"]
#[allow(dead_code)]
mod ime;
#[cfg(any(windows, test))]
#[path = "../focus/intervention.rs"]
mod intervention;
#[cfg(any(windows, test))]
#[path = "../logic.rs"]
#[allow(dead_code)]
mod logic;
#[cfg(any(windows, test))]
#[path = "../model.rs"]
mod model;
#[cfg(any(windows, test))]
#[path = "../focus/policy.rs"]
#[allow(dead_code)]
mod policy;
#[cfg(any(windows, test))]
#[path = "../auto_focus/rules.rs"]
#[cfg_attr(not(windows), allow(dead_code))]
mod rules;
#[cfg(any(windows, test))]
#[path = "../auto_send/transition.rs"]
mod transition;
#[cfg(any(windows, test))]
#[path = "../types.rs"]
#[allow(dead_code)]
mod types;
#[cfg(windows)]
#[path = "../auto_send/uia.rs"]
mod uia;
#[cfg(any(windows, test))]
#[path = "../auto_send/writer.rs"]
mod writer;

/// 通常版とは別のコンソールで登録なしでフォーカス確認後の一回送信を試す。
fn main() {
    #[cfg(windows)]
    {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        let inspect = match mode::parse(&args) {
            Ok(value) => value,
            Err(()) => {
                eprintln!("Usage: iris-auto-send-probe.exe [--inspect]");
                std::process::exit(2);
            }
        };
        INSPECT_ONLY.set(inspect).expect("mode initialized once");
        println!(
            "mode={} no_focus_text_enter_ime_writes={inspect}",
            if inspect { "inspect" } else { "send" }
        );
        app::run();
    }
    #[cfg(not(windows))]
    {
        eprintln!("Iris auto-send probe は Windows 専用です。");
        std::process::exit(1);
    }
}
