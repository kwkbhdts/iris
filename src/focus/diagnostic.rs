use std::{fmt, sync::OnceLock, time::Instant};

static CLOCK: OnceLock<Instant> = OnceLock::new();

/// フック登録前に共通の単調時計の原点を決める。
pub fn initialize_clock() {
    CLOCK.get_or_init(Instant::now);
}

/// 同じ Instant 時計の時刻を固定長記録へ変換する。不明は番兵値にする。
pub fn mono_ns(at: Instant) -> u64 {
    CLOCK
        .get()
        .and_then(|origin| at.checked_duration_since(*origin))
        .and_then(|elapsed| u64::try_from(elapsed.as_nanos()).ok())
        .unwrap_or(u64::MAX)
}

/// 異なる時計は混ぜず、共通原点の時刻だけを要求からの相対usへ直す。
pub fn relative_mono_us(value: u64, origin: u64) -> Option<i128> {
    if value == u64::MAX || origin == u64::MAX {
        None
    } else {
        Some((i128::from(value) - i128::from(origin)) / 1000)
    }
}

const MAX_LINES: usize = 320;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum App {
    Claude,
    ChatGpt,
    Firefox,
    Chrome,
}
impl App {
    /// 許可された名前だけを固定ラベルへ変換し、元のパスは保持しない。
    pub fn from_path(path: &str) -> Option<Self> {
        let name = path.rsplit(['\\', '/']).next()?;
        [Self::Claude, Self::ChatGpt, Self::Firefox, Self::Chrome]
            .into_iter()
            .find(|app| name.eq_ignore_ascii_case(app.name()))
    }

    /// 診断には検証済みの固定 exe 名だけを出す。
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude.exe",
            Self::ChatGpt => "ChatGPT.exe",
            Self::Firefox => "firefox.exe",
            Self::Chrome => "chrome.exe",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TargetScope {
    RegisteredFocus,
    ReadOnlyDiscovery,
    ChatGptOnly,
    ClaudePath,
    AutoSend,
    CandidateCount,
}

impl TargetScope {
    /// 読み取り診断の拡張を、フォーカス・送信試作の許可へ波及させない。
    pub fn allows(self, app: App) -> bool {
        match self {
            Self::RegisteredFocus | Self::CandidateCount => true,
            Self::ReadOnlyDiscovery => matches!(app, App::ChatGpt | App::Claude),
            Self::ChatGptOnly => app == App::ChatGpt,
            Self::ClaudePath => app == App::Claude,
            Self::AutoSend => true,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Context {
    pub id: u64,
    pub register: bool,
    pub app: App,
    pub started: Instant,
}
impl Context {
    /// 固定メタデータと経過時間を各行に付ける。
    pub fn line(self, stage: &'static str, detail: fmt::Arguments<'_>) -> String {
        let elapsed = self.started.elapsed();
        format!(
            "req={} key={} app={} stage={} ms={} mono_us={} {}",
            self.id,
            if self.register { "F2" } else { "F1" },
            self.app.name(),
            stage,
            elapsed.as_millis(),
            elapsed.as_micros(),
            detail
        )
    }
}

pub struct Trace {
    pub context: Context,
    pub stage: &'static str,
    lines: Vec<String>,
    truncated: bool,
}
impl Trace {
    /// UIA 処理中のコンソール出力を避け、有界の記録を用意する。
    pub fn new(context: Context) -> Self {
        Self {
            context,
            stage: "begin",
            lines: Vec::new(),
            truncated: false,
        }
    }

    /// 内容由来の文字列を渡さず、数値・真偽値・固定ラベルだけを記録する。
    pub fn record(&mut self, detail: fmt::Arguments<'_>) {
        if self.lines.len() < MAX_LINES {
            self.lines.push(self.context.line(self.stage, detail));
        } else {
            self.truncated = true;
        }
    }

    /// 診断上限に達しても最終結果を必ず付けて一組として出力する。
    pub fn finish(mut self, result: &'static str, stopped: bool) -> String {
        self.lines.push(self.context.line(
            "end",
            format_args!(
                "result={result} stopped={stopped} truncated={}",
                self.truncated
            ),
        ));
        self.lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// パス・似た名前・未知のアプリを診断へ流さない。
    #[test]
    fn only_fixed_app_labels_are_retained() {
        assert_eq!(
            App::from_path(r"C:\private\FIREFOX.EXE"),
            Some(App::Firefox)
        );
        for name in [
            "firefox.exe.bak",
            "mychrome.exe",
            "codex.exe",
            "",
            "ChatGPT",
        ] {
            assert_eq!(App::from_path(name), None);
        }
        assert_eq!(App::from_path("ChatGPT.exe").unwrap().name(), "ChatGPT.exe");
    }

    /// 診断上限後も文脈と最終結果を失わない。
    #[test]
    fn bounded_trace_keeps_terminal_result() {
        let context = Context {
            id: 7,
            register: true,
            app: App::Chrome,
            started: Instant::now(),
        };
        let mut trace = Trace::new(context);
        for _ in 0..MAX_LINES + 10 {
            trace.record(format_args!("type=50004"));
        }
        let output = trace.finish("failed", false);
        assert_eq!(output.lines().count(), MAX_LINES + 1);
        assert!(output
            .lines()
            .all(|line| line.starts_with("req=7 key=F2 app=chrome.exe stage=")));
        assert!(output
            .lines()
            .last()
            .unwrap()
            .contains("result=failed stopped=false truncated=true"));
    }
    /// 共通時計は実測 Instant の差と一致し、粗いtickと混ぜない。
    #[test]
    fn monotonic_offsets_share_the_request_origin() {
        initialize_clock();
        let start = Instant::now();
        let end = start + std::time::Duration::from_micros(19500);
        assert_eq!(relative_mono_us(mono_ns(end), mono_ns(start)), Some(19500));
        assert_eq!(relative_mono_us(mono_ns(start), mono_ns(end)), Some(-19500));
        assert_eq!(relative_mono_us(u64::MAX, 0), None);
    }
    /// 読み取り診断の対象と、登録型・送信試作の制限を別々に固定する。
    #[test]
    fn target_scopes_keep_browser_and_write_paths_separate() {
        for app in [App::Claude, App::ChatGpt, App::Chrome, App::Firefox] {
            assert!(TargetScope::RegisteredFocus.allows(app));
            assert!(TargetScope::CandidateCount.allows(app));
            assert_eq!(
                TargetScope::ReadOnlyDiscovery.allows(app),
                matches!(app, App::Claude | App::ChatGpt)
            );
            assert_eq!(TargetScope::ChatGptOnly.allows(app), app == App::ChatGpt);
            assert_eq!(TargetScope::ClaudePath.allows(app), app == App::Claude);
            assert!(TargetScope::AutoSend.allows(app));
        }
    }

    /// 各exeで実際に選んだ許可リストもバイナリ名に対して固定する。
    #[test]
    fn binary_uses_its_expected_target_scope() {
        let expected = match env!("CARGO_BIN_NAME") {
            "iris-focus-probe" => TargetScope::RegisteredFocus,
            "iris-discovery-probe" => TargetScope::ReadOnlyDiscovery,
            "iris-path-probe" => TargetScope::ClaudePath,
            "iris-auto-send-probe" => TargetScope::AutoSend,
            "iris-candidate-probe" => TargetScope::CandidateCount,
            _ => TargetScope::ChatGptOnly,
        };
        assert_eq!(crate::TARGET_SCOPE, expected);
    }
}
