use crate::diagnostic::App;
use std::ffi::OsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Send,
    Inspect,
    ObserveIme,
    NoImeChatGpt,
    NoImeDesktop,
}
impl Mode {
    /// 明示比較だけを指定アプリに限定し、ブラウザーへ広げない。
    pub fn allows(self, app: App) -> bool {
        match self {
            Self::NoImeChatGpt => app == App::ChatGpt,
            Self::NoImeDesktop => matches!(app, App::ChatGpt | App::Claude),
            _ => true,
        }
    }
    /// 互換指定とデスクトップ指定は、同じ一回送信経路を使う。
    pub fn uses_no_ime(self) -> bool {
        matches!(self, Self::NoImeChatGpt | Self::NoImeDesktop)
    }
    /// 起動時の固定モード名を返す。
    pub fn label(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Inspect => "inspect",
            Self::ObserveIme => "observe-ime",
            Self::NoImeChatGpt => "no-ime-chatgpt",
            Self::NoImeDesktop => "no-ime-desktop",
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
        [arg] if arg == "--no-ime-desktop" => Ok(Mode::NoImeDesktop),
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
        for (flag, mode, allowed, no_ime) in [
            ("--inspect", Mode::Inspect, [true, true, true, true], false),
            (
                "--observe-ime",
                Mode::ObserveIme,
                [true, true, true, true],
                false,
            ),
            (
                "--no-ime-chatgpt",
                Mode::NoImeChatGpt,
                [true, false, false, false],
                true,
            ),
            (
                "--no-ime-desktop",
                Mode::NoImeDesktop,
                [true, true, false, false],
                true,
            ),
        ] {
            assert_eq!(parse(&[flag.into()]), Ok(mode));
            assert!(!mode.label().is_empty());
            assert_eq!(mode.uses_no_ime(), no_ime);
            for other in [
                "--inspect",
                "--observe-ime",
                "--no-ime-chatgpt",
                "--no-ime-desktop",
            ] {
                assert!(parse(&[flag.into(), other.into()]).is_err());
            }
            for (app, expected) in [App::ChatGpt, App::Claude, App::Firefox, App::Chrome]
                .into_iter()
                .zip(allowed)
            {
                assert_eq!(mode.allows(app), expected, "{mode:?} {app:?}");
            }
        }
        assert!(!Mode::Send.uses_no_ime());
        for app in [App::ChatGpt, App::Claude, App::Firefox, App::Chrome] {
            assert!(Mode::Send.allows(app));
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
