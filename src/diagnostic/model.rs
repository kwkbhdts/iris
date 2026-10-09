pub const CAPTURE_TIMEOUT_MS: u32 = 500;

#[derive(Clone, Copy, Default)]
pub struct F1Gate {
    pub down: bool,
    swallowed: bool,
}

impl F1Gate {
    pub const EMPTY: Self = Self {
        down: false,
        swallowed: false,
    };

    /// 単独 F1 の最初の押下だけ採取し、長押しと待機中の連打を抑止する。
    pub fn event(&mut self, down: bool, modified: bool, busy: bool) -> (bool, bool) {
        if !down {
            let swallowed = self.swallowed;
            *self = Self::EMPTY;
            return (swallowed, false);
        }
        if self.down {
            return (self.swallowed, false);
        }
        self.down = true;
        self.swallowed = !modified;
        (self.swallowed, !modified && !busy)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowId {
    pub hwnd: usize,
    pub pid: u32,
    pub tid: u32,
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub id: usize,
    pub at: u32,
    pub foreground: Result<WindowId, u32>,
    pub focus: Result<WindowId, u32>,
}

impl Snapshot {
    /// 遅れた結果や別の採取の結果を表示しない。
    pub fn accepts(&self, id: usize, now: u32) -> bool {
        self.id == id && now.wrapping_sub(self.at) <= CAPTURE_TIMEOUT_MS
    }
}

#[derive(Clone, Copy)]
pub enum ImeReply {
    NotQueried,
    Failed(u32),
    Raw(usize),
}

impl ImeReply {
    /// 配送成功と IME 状態の確定を区別し、ゼロ応答も生値のまま示す。
    pub fn describe(self) -> String {
        match self {
            Self::NotQueried => "not_queried; interpretation=unknown".into(),
            Self::Failed(code) => {
                format!("failed_or_timed_out; error={code}; interpretation=unknown")
            }
            Self::Raw(value) => {
                format!("message_delivered; raw=0x{value:X}; interpretation=unverified")
            }
        }
    }
}

/// HWND の識別情報だけを表示し、ウィンドウのタイトルは読まない。
pub fn identity(value: Result<WindowId, u32>) -> String {
    match value {
        Ok(w) => format!("hwnd=0x{:X}; pid={}; tid={}", w.hwnd, w.pid, w.tid),
        Err(code) => format!("unavailable; error={code}"),
    }
}

/// プロセス名の改行等が診断結果の別の行に見えないようにする。
pub fn process_label(value: &Result<String, u32>) -> String {
    match value {
        Ok(name) => name.escape_debug().to_string(),
        Err(code) => format!("unavailable; error={code}"),
    }
}

/// 診断の IME 問い合わせは今回の対象アプリに限定する。
pub fn supported(name: &Result<String, u32>) -> bool {
    name.as_ref().is_ok_and(|s| {
        s.eq_ignore_ascii_case("firefox.exe") || s.eq_ignore_ascii_case("Claude.exe")
    })
}

pub struct Report {
    pub snapshot: Snapshot,
    pub process: Result<String, u32>,
    pub focus_process: Result<String, u32>,
    pub ime_window: Result<WindowId, u32>,
    pub layout: usize,
    pub open: ImeReply,
    pub conversion: ImeReply,
    pub status: &'static str,
    pub elapsed_ms: u32,
}

impl Report {
    /// 識別情報と状態応答だけで貼り付け用の結果を作る。
    pub fn render(&self) -> String {
        format!(
            "--- sample #{} ---\nstatus={}; elapsed_ms={}\nforeground_at_f1: {}; process={}\nfocus_at_f1: {}; process={}\nkeyboard_layout_raw=0x{:X}\nime_window: {}\nlegacy_open: {}\nlegacy_conversion: {}\nactual_tsf_open=unknown\ncomposition=unknown (not_queried; no text collected)\n",
            self.snapshot.id, self.status, self.elapsed_ms,
            identity(self.snapshot.foreground), process_label(&self.process),
            identity(self.snapshot.focus), process_label(&self.focus_process),
            self.layout, identity(self.ime_window), self.open.describe(), self.conversion.describe(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 長押し・待機中の連打から追加の採取を発生させない。
    #[test]
    fn f1_captures_once_and_suppresses_busy_presses() {
        let mut gate = F1Gate::EMPTY;
        assert_eq!(gate.event(true, false, false), (true, true));
        assert_eq!(gate.event(true, false, false), (true, false));
        assert_eq!(gate.event(false, false, false), (true, false));
        assert_eq!(gate.event(true, false, true), (true, false));
        assert_eq!(gate.event(true, false, false), (true, false));
        assert_eq!(gate.event(false, false, false), (true, false));
        assert_eq!(gate.event(true, false, false), (true, true));
    }

    /// 修飾付きや起動前から押されている F1 は横取りしない。
    #[test]
    fn modified_and_preheld_f1_pass() {
        let mut gate = F1Gate::EMPTY;
        assert_eq!(gate.event(true, true, false), (false, false));
        assert_eq!(gate.event(true, false, false), (false, false));
        assert_eq!(gate.event(false, false, false), (false, false));
        gate.down = true;
        assert_eq!(gate.event(true, false, false), (false, false));
    }

    /// 期限切れ・古い採取の結果を拒否し、時計の周回も扱う。
    #[test]
    fn late_or_stale_results_are_rejected() {
        let s = Snapshot {
            id: 3,
            at: u32::MAX - 10,
            foreground: Err(0),
            focus: Err(0),
        };
        assert!(s.accepts(3, s.at.wrapping_add(500)));
        assert!(!s.accepts(3, s.at.wrapping_add(501)));
        assert!(!s.accepts(2, s.at.wrapping_add(10)));
    }

    /// ゼロ応答・失敗・未取得を IME オフや変換なしと断定しない。
    #[test]
    fn ime_replies_preserve_uncertainty() {
        assert!(ImeReply::Raw(0)
            .describe()
            .contains("raw=0x0; interpretation=unverified"));
        assert!(ImeReply::Raw(1)
            .describe()
            .contains("raw=0x1; interpretation=unverified"));
        assert!(ImeReply::Failed(0)
            .describe()
            .contains("interpretation=unknown"));
        assert!(ImeReply::NotQueried
            .describe()
            .contains("interpretation=unknown"));
    }

    /// 対象名は厳密に比較し、表示する名前の制御文字は無害化する。
    #[test]
    fn process_names_are_bounded_to_the_expected_role() {
        assert!(supported(&Ok("FIREFOX.EXE".into())));
        assert!(supported(&Ok("Claude.exe".into())));
        assert!(!supported(&Ok("not-firefox.exe".into())));
        assert!(!supported(&Err(5)));
        assert_eq!(process_label(&Ok("a\nb.exe".into())), "a\\nb.exe");
    }

    /// 結果は変換中文字列や状態の断定を含まない。
    #[test]
    fn report_marks_composition_and_tsf_unknown() {
        let r = Report {
            snapshot: Snapshot {
                id: 1,
                at: 0,
                foreground: Err(5),
                focus: Err(5),
            },
            process: Err(5),
            focus_process: Err(5),
            ime_window: Err(0),
            layout: 0,
            open: ImeReply::Raw(0),
            conversion: ImeReply::Raw(0),
            status: "sampled",
            elapsed_ms: 10,
        }
        .render();
        assert!(r.contains("composition=unknown"));
        assert!(r.contains("actual_tsf_open=unknown"));
        assert!(!r.contains("composition=none"));
        assert!(!r.contains("ime_open=false"));
        assert!(identity(Ok(WindowId {
            hwnd: 1,
            pid: 2,
            tid: 3
        }))
        .contains("pid=2"));
    }
}
