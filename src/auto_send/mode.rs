use crate::diagnostic::App;
use std::ffi::OsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Send,
    Inspect,
    ObserveIme,
    NoImeChatGpt,
}
impl Mode {
    /// 比較方式はChatGPTの明示指定だけに限定する。
    pub fn allows(self, app: App) -> bool {
        self != Self::NoImeChatGpt || app == App::ChatGpt
    }
    /// 起動時の固定モード名を返す。
    pub fn label(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Inspect => "inspect",
            Self::ObserveIme => "observe-ime",
            Self::NoImeChatGpt => "no-ime-chatgpt",
        }
    }
}
/// 引数未指定を維持し、誤記や組合せで別方式へ戻さない。
pub fn parse(args: &[OsString]) -> Result<Mode, ()> {
    match args {
        [] => Ok(Mode::Send),
        [arg] if arg == "--inspect" => Ok(Mode::Inspect),
        [arg] if arg == "--observe-ime" => Ok(Mode::ObserveIme),
        [arg] if arg == "--no-ime-chatgpt" => Ok(Mode::NoImeChatGpt),
        _ => Err(()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// 既定・各明示指定・アプリ制限と、誤記・複数指定拒否を確認する。
    #[test]
    fn explicit_modes_and_scope() {
        assert_eq!(parse(&[]), Ok(Mode::Send));
        for (flag, mode) in [
            ("--inspect", Mode::Inspect),
            ("--observe-ime", Mode::ObserveIme),
            ("--no-ime-chatgpt", Mode::NoImeChatGpt),
        ] {
            assert_eq!(parse(&[flag.into()]), Ok(mode));
            assert!(!mode.label().is_empty());
            for other in ["--inspect", "--observe-ime", "--no-ime-chatgpt"] {
                assert!(parse(&[flag.into(), other.into()]).is_err());
            }
            for app in [App::ChatGpt, App::Claude, App::Firefox, App::Chrome] {
                assert_eq!(
                    mode.allows(app),
                    mode != Mode::NoImeChatGpt || app == App::ChatGpt
                );
            }
        }
        for arg in ["--send", "--no-ime", "--inspect-only"] {
            assert!(parse(&[arg.into()]).is_err());
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            assert!(parse(&[OsString::from_vec(vec![0xff])]).is_err());
        }
    }
}
