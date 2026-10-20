use crate::{
    app::{self, Request},
    comparison::{self, Timeline},
    diagnostic::Trace,
    failure::{self, Identity},
    ime,
    model::ImePhase,
    types::{ImeReply, Snapshot, WindowId},
};
use std::time::{Duration, Instant};

pub struct Recorder {
    request: Request,
    permitted: Option<[u64; 2]>,
    saved: (Snapshot, WindowId, usize),
    epoch: u64,
    pub restored_at: Option<Instant>,
    entries: Vec<String>,
    post: bool,
    reason: &'static str,
    unknown: bool,
}
impl Recorder {
    /// 準備時の保存先と元履歴を固定し、後から採り直さない。
    pub fn new(
        request: Request,
        permitted: Option<[u64; 2]>,
        saved: (Snapshot, WindowId, usize),
        epoch: u64,
    ) -> Self {
        Self {
            request,
            permitted,
            saved,
            epoch,
            restored_at: None,
            entries: vec![],
            post: false,
            reason: "none",
            unknown: false,
        }
    }
    /// 観測と元要求の期限を混ぜず、介入・対象変更時は読取りも止める。
    fn guard(&mut self) -> bool {
        let result = if self.post {
            app::check_post_observation(self.request, self.permitted)
        } else {
            app::check_send(self.request, self.permitted)
        };
        if let Err(reason) = result {
            self.reason = reason;
            return false;
        }
        if !ime::observation_history_valid(self.epoch) {
            self.reason = "ime_history_or_stop";
            return false;
        }
        if self.post
            && self.restored_at.is_some_and(|at| {
                at.elapsed().as_millis() > u128::from(comparison::OBSERVATION_BUDGET_MS)
            })
        {
            self.reason = "observation_deadline";
            return false;
        }
        true
    }
    /// 数値識別子を出さず、保存操作先との既知一致だけを返す。
    fn identity(&self) -> [Option<bool>; 5] {
        let (foreground, focus, ime, layout) = ime::read_identity();
        failure::compare(
            Identity {
                foreground: self.saved.0.foreground.ok(),
                focus: self.saved.0.focus.ok(),
                ime: Some(self.saved.1),
                layout: Some(self.saved.2),
            },
            Identity {
                foreground,
                focus,
                ime,
                layout,
            },
        )
    }
    /// conversionの生値は観測に留め、対応保証や日本語復帰を宣言しない。
    fn capture(&mut self, label: &str, open: ImeReply) -> bool {
        let at = self.request.started.elapsed().as_millis();
        if !self.guard() {
            self.entries.push(format!(
                "phase={label} observed_ms={at} raw_reads_complete=false reason={}",
                self.reason
            ));
            return false;
        }
        let before = self.identity();
        let valid_before = before == [Some(true); 5] && self.guard();
        let conversion = if valid_before {
            ime::read_conversion(self.saved.1)
        } else {
            ImeReply::NotQueried
        };
        let after = if valid_before && self.guard() {
            self.identity()
        } else {
            [None; 5]
        };
        let valid = valid_before && after == [Some(true); 5] && self.guard();
        if !valid && self.reason == "none" {
            self.reason = "saved_identity_changed_or_unknown";
        }
        let complete = comparison::raw_reads_complete(valid, open, conversion);
        self.unknown |= !complete;
        let conversion = comparison::conversion_label(conversion);
        self.entries.push(format!("phase={label} observed_ms={at} legacy_open={} conversion_raw={conversion} conversion_semantics=unverified foreground_same={} win32_focus_same={} ime_window_same={} ime_owner_same={} layout_same={} raw_reads_complete={complete} actual_tsf_state=unknown reason={}",comparison::open_label(open),failure::label(after[0]),failure::label(after[1]),failure::label(after[2]),failure::label(after[3]),failure::label(after[4]),self.reason));
        valid
    }
    /// 元モデルの三つの段階でのみ追加観測する。
    pub fn phase(&mut self, phase: ImePhase, open: ImeReply) -> bool {
        if !self.guard() {
            return false;
        }
        let label = match phase {
            ImePhase::Start => "start",
            ImePhase::Off => "after_off",
            ImePhase::Restored => {
                self.restored_at = Some(Instant::now());
                "immediate_restore_readback"
            }
        };
        self.capture(label, open)
    }
    /// 全書込み終了後に一方向で読取りへ移り、復元・入力を再実行しない。
    pub fn follow(&mut self, trace: &mut Trace) {
        if let Err(reason) = app::begin_post_observation(self.request, self.permitted) {
            self.reason = reason;
        } else {
            self.post = true;
            if comparison::follow(self) {
                self.reason = if self.unknown {
                    "completed_with_unknown_values"
                } else {
                    "raw_observations_completed"
                };
            } else if self.reason == "none" {
                self.reason = "observation_incomplete_or_deadline";
            }
        }
        self.flush(trace);
        trace.stage = "ime.observation_end";
        trace.record(format_args!(
            "reason={} read_only=true no_retry=true app_and_tsf_result=unknown",
            self.reason
        ));
    }
    /// 元要求とサンプル時刻を一緒に記録し、本文やURLは扱わない。
    pub fn flush(&mut self, trace: &mut Trace) {
        trace.stage = "ime.observe";
        for entry in self.entries.drain(..) {
            trace.record(format_args!("{entry}"));
        }
    }
}
impl Timeline for Recorder {
    /// オン読戻し直後を原点とし、サンプル処理時間を加算しない。
    fn elapsed(&self) -> u32 {
        self.restored_at.map_or(u32::MAX, |t| {
            t.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
        })
    }
    /// 各待機区間でも保存先と元履歴を再確認する。
    fn check(&mut self) -> bool {
        if !self.guard() {
            return false;
        }
        if self.identity() != [Some(true); 5] {
            self.reason = "saved_identity_changed_or_unknown";
            return false;
        }
        true
    }
    /// 書込みのない短い待機だけを行う。
    fn wait(&mut self) {
        std::thread::sleep(Duration::from_millis(5));
    }
    /// 指定オフセットにopenとconversionを一回ずつ読む。
    fn sample(&mut self, offset: u32) -> bool {
        if !self.check() {
            return false;
        }
        let open = ime::read_saved_open(self.saved.1);
        self.capture(&format!("restore_plus_{offset}ms"), open)
    }
}
