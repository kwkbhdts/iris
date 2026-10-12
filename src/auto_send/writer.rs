use crate::{
    model::{self, Backend, InputReply},
    types::ImeReply,
};
use std::time::{Duration, Instant};

/// 既存IMEバックエンドへ固定世代ガードと送信直前のUIA検証を接続する。
pub struct Writer<'a, B: Backend> {
    pub inner: B,
    pub quick: &'a dyn Fn() -> bool,
    pub validate: &'a mut dyn FnMut() -> bool,
    pub failed: bool,
    pub consumed: bool,
}
impl<B: Backend> Backend for Writer<'_, B> {
    /// 一度の検証失敗を保持し、同じ世代に見えても再開しない。
    fn guarded(&mut self) -> bool {
        if self.failed {
            return false;
        }
        if !(self.quick)() || !self.inner.guarded() || !(self.quick)() {
            self.failed = true;
        }
        !self.failed
    }
    /// 終了や期限切れでは新しいIMEオフ・入力を開始しない。
    fn closing(&self) -> bool {
        !(self.quick)() || self.inner.closing()
    }
    /// 保存したIMEへの既存の開閉問い合わせを使う。
    fn read_open(&mut self) -> ImeReply {
        self.inner.read_open()
    }
    /// 保存したIMEへの既存の開閉要求を使う。
    fn set_open(&mut self, open: bool) -> ImeReply {
        self.inner.set_open(open)
    }
    /// IMEオフ後に対象を再探索し、通った場合だけ既存の6イベント一括送信を呼ぶ。
    fn send_once(&mut self) -> InputReply {
        if self.consumed || self.closing() || !self.guarded() {
            return InputReply {
                attempted: false,
                inserted: 0,
            };
        }
        self.consumed = true;
        if !(self.validate)() {
            self.failed = true;
        }
        if !self.guarded() {
            return InputReply {
                attempted: false,
                inserted: 0,
            };
        }
        self.inner.send_once()
    }
    /// 復元前の既存100ms待機中にも、新しい世代と元の期限を確認する。
    fn wait_before_restore(&mut self) {
        let start = Instant::now();
        while start.elapsed().as_millis() < model::RESTORE_DELAY_MS as u128 {
            if !self.guarded() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    struct Fake<'a> {
        active: &'a Cell<bool>,
        on: bool,
        abort_after_off: bool,
        sends: usize,
        inserted: u32,
    }
    impl Backend for Fake<'_> {
        fn guarded(&mut self) -> bool {
            self.active.get()
        }
        fn closing(&self) -> bool {
            false
        }
        fn read_open(&mut self) -> ImeReply {
            ImeReply::Raw(usize::from(self.on))
        }
        fn set_open(&mut self, open: bool) -> ImeReply {
            self.on = open;
            if !open && self.abort_after_off {
                self.active.set(false);
            }
            ImeReply::Raw(0)
        }
        fn send_once(&mut self) -> InputReply {
            self.sends += 1;
            InputReply {
                attempted: true,
                inserted: self.inserted,
            }
        }
        fn wait_before_restore(&mut self) {}
    }
    /// IMEオフ後の介入や送信直前の候補差し替えでは、一度も投入しない。
    #[test]
    fn intervention_after_ime_and_candidate_change_block_input() {
        for (abort_after_off, candidate_same) in [(true, true), (false, false)] {
            let active = Cell::new(true);
            let quick = || active.get();
            let mut validate = || candidate_same;
            let inner = Fake {
                active: &active,
                on: true,
                abort_after_off,
                sends: 0,
                inserted: 6,
            };
            let mut writer = Writer {
                inner,
                quick: &quick,
                validate: &mut validate,
                failed: false,
                consumed: false,
            };
            let result = model::run(&mut writer);
            assert_eq!(writer.inner.sends, 0);
            assert!(result.manual);
        }
    }
    /// 検証中の介入は最終ガードで止まり、元に戻っても再開しない。
    #[test]
    fn intervention_during_validation_latches_failure() {
        let active = Cell::new(true);
        let quick = || active.get();
        let mut validate = || {
            active.set(false);
            true
        };
        let inner = Fake {
            active: &active,
            on: false,
            abort_after_off: false,
            sends: 0,
            inserted: 6,
        };
        let mut writer = Writer {
            inner,
            quick: &quick,
            validate: &mut validate,
            failed: false,
            consumed: false,
        };
        assert!(!writer.send_once().attempted);
        active.set(true);
        assert!(!writer.send_once().attempted);
        assert_eq!(writer.inner.sends, 0);
    }
    /// 部分投入でモデルもアダプターも再送せず、完全投入にも二回目はない。
    #[test]
    fn partial_and_complete_input_are_each_attempted_once() {
        for inserted in [0, 3, 6] {
            let active = Cell::new(true);
            let quick = || active.get();
            let mut validate = || true;
            let inner = Fake {
                active: &active,
                on: false,
                abort_after_off: false,
                sends: 0,
                inserted,
            };
            let mut writer = Writer {
                inner,
                quick: &quick,
                validate: &mut validate,
                failed: false,
                consumed: false,
            };
            let result = model::run(&mut writer);
            assert_eq!(result.input.unwrap().inserted, inserted);
            assert_eq!(result.manual, inserted != 6);
            assert!(!writer.send_once().attempted);
            assert_eq!(writer.inner.sends, 1);
        }
    }
}
