use crate::diagnostic::App;
use std::ffi::OsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Send,
    Inspect,
    ObserveIme,
    NoImeChatGpt,
    NoImeDesktop,
    NoImeDesktopTwoEnters,
    NoImeBrowserTwoEnters,
}
impl Mode {
    /// 明示比較を、それぞれ指定したアプリだけに限定する。
    pub fn allows(self, app: App) -> bool {
        match self {
            Self::NoImeChatGpt => app == App::ChatGpt,
            Self::NoImeDesktop | Self::NoImeDesktopTwoEnters => {
                matches!(app, App::ChatGpt | App::Claude)
            }
            Self::NoImeBrowserTwoEnters => matches!(app, App::Firefox | App::Chrome),
            _ => true,
        }
    }
    /// IME非操作の明示指定は、共通の一回投入経路を使う。
    pub fn uses_no_ime(self) -> bool {
        matches!(
            self,
            Self::NoImeChatGpt
                | Self::NoImeDesktop
                | Self::NoImeDesktopTwoEnters
                | Self::NoImeBrowserTwoEnters
        )
    }
    /// 既存指定は6イベント、新しい明示比較だけ8イベントにする。
    pub fn input_plan(self) -> crate::model::InputPlan {
        if matches!(
            self,
            Self::NoImeDesktopTwoEnters | Self::NoImeBrowserTwoEnters
        ) {
            crate::model::InputPlan::TwoEnters
        } else {
            crate::model::InputPlan::SingleEnter
        }
    }
    /// 起動時の固定モード名を返す。
    pub fn label(self) -> &'static str {
        match self {
            Self::Send => "send",
            Self::Inspect => "inspect",
            Self::ObserveIme => "observe-ime",
            Self::NoImeChatGpt => "no-ime-chatgpt",
            Self::NoImeDesktop => "no-ime-desktop",
            Self::NoImeDesktopTwoEnters => "no-ime-desktop-two-enters",
            Self::NoImeBrowserTwoEnters => "no-ime-browser-two-enters",
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
        [arg] if arg == "--no-ime-desktop-two-enters" => Ok(Mode::NoImeDesktopTwoEnters),
        [arg] if arg == "--no-ime-browser-two-enters" => Ok(Mode::NoImeBrowserTwoEnters),
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
            (
                "--no-ime-desktop-two-enters",
                Mode::NoImeDesktopTwoEnters,
                [true, true, false, false],
                true,
            ),
            (
                "--no-ime-browser-two-enters",
                Mode::NoImeBrowserTwoEnters,
                [false, false, true, true],
                true,
            ),
        ] {
            assert_eq!(parse(&[flag.into()]), Ok(mode));
            assert!(!mode.label().is_empty());
            assert_eq!(mode.uses_no_ime(), no_ime);
            assert_eq!(
                mode.input_plan().count(),
                if matches!(
                    flag,
                    "--no-ime-desktop-two-enters" | "--no-ime-browser-two-enters"
                ) {
                    8
                } else {
                    6
                }
            );
            for other in [
                "--inspect",
                "--observe-ime",
                "--no-ime-chatgpt",
                "--no-ime-desktop",
                "--no-ime-desktop-two-enters",
                "--no-ime-browser-two-enters",
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
        assert_eq!(Mode::Send.input_plan().count(), 6);
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
