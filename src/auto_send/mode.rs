use std::ffi::OsString;

/// 明示した読み取り指定だけを認め、誤記や余分な引数で送信へ戻さない。
pub fn parse(args: &[OsString]) -> Result<bool, ()> {
    match args {
        [] => Ok(false),
        [arg] if arg == "--inspect" => Ok(true),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 読み取り指定の誤記・重複・非UTF8を送信モードとして起動しない。
    #[test]
    fn mode_is_explicit_and_rejects_unknown_arguments() {
        assert_eq!(parse(&[]), Ok(false));
        assert_eq!(parse(&["--inspect".into()]), Ok(true));
        for args in [
            vec!["--inspect-only".into()],
            vec!["--inspect".into(), "--inspect".into()],
            vec!["--send".into()],
        ] {
            assert_eq!(parse(&args), Err(()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            assert_eq!(parse(&[OsString::from_vec(vec![0xff])]), Err(()));
        }
    }
}
