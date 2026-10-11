use std::{fmt, time::Instant};

const MAX_LINES: usize = 192;

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
        format!(
            "req={} key={} app={} stage={} ms={} {}",
            self.id,
            if self.register { "F2" } else { "F1" },
            self.app.name(),
            stage,
            self.started.elapsed().as_millis(),
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
}
