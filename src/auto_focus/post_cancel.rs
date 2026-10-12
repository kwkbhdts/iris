use crate::intervention::{Kind, Snapshot};
pub type Result<T> = std::result::Result<T, &'static str>;

/// 原要求以降の履歴が欠落なく、前面通知だけの場合に限る。
pub fn foreground_only(snapshot: &Snapshot) -> bool {
    !snapshot.events.is_empty()
        && snapshot.missing == 0
        && snapshot.total == Some(snapshot.events.len() as u64)
        && snapshot
            .events
            .iter()
            .all(|event| event.observation.kind == Kind::Foreground)
}

/// 操作を再開する能力を持たない、フォーカス・型・同一性の読み取りだけの観測口。
pub trait Reader {
    type Element;
    fn check(&mut self) -> Result<()>;
    fn focused(&mut self) -> Result<Self::Element>;
    fn control_type(&mut self, element: &Self::Element) -> Result<i32>;
    fn matches(&mut self, element: &Self::Element) -> Result<bool>;
}

#[derive(Debug, PartialEq, Eq)]
pub struct Sample {
    pub control_type: i32,
    pub matched: bool,
}

/// API前・間・後を確認し、一組だけ観測する。途中の変化は一致を返さない。
pub fn once(reader: &mut impl Reader) -> Result<Sample> {
    reader.check()?;
    let element = reader.focused()?;
    reader.check()?;
    let control_type = reader.control_type(&element)?;
    reader.check()?;
    let matched = reader.matches(&element)?;
    reader.check()?;
    Ok(Sample {
        control_type,
        matched,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intervention::{Event, Observation, Truth};

    fn event(kind: Kind) -> Event {
        Event {
            epoch: 1,
            observation: Observation {
                kind,
                injected: Truth::Unknown,
                event_tick: 0,
                received_tick: 0,
                received_mono_ns: 0,
                window: 0,
                window_pid: 0,
                same_received_foreground: Truth::Unknown,
            },
        }
    }
    /// 不明・欠落・空履歴・別種介入では観測を許可しない。
    #[test]
    fn eligibility_requires_complete_foreground_history() {
        let mut s = Snapshot {
            total: Some(1),
            missing: 0,
            events: vec![event(Kind::Foreground)],
        };
        assert!(foreground_only(&s));
        for kind in [
            Kind::MouseMove,
            Kind::MouseButton,
            Kind::MouseWheel,
            Kind::MouseOther,
            Kind::KeyboardOther,
            Kind::NewShortcut,
            Kind::Watchdog,
        ] {
            s.events[0] = event(kind);
            assert!(!foreground_only(&s));
        }
        s.events[0] = event(Kind::Foreground);
        s.missing = 1;
        assert!(!foreground_only(&s));
        s.missing = 0;
        s.total = None;
        assert!(!foreground_only(&s));
        s.total = Some(2);
        assert!(!foreground_only(&s));
        s.events.clear();
        s.total = Some(0);
        assert!(!foreground_only(&s));
    }
    struct Fake {
        checks: usize,
        fail_check: usize,
        gets: usize,
        compares: usize,
        types: usize,
        type_error: bool,
        get_error: bool,
        compare_error: bool,
    }
    impl Reader for Fake {
        type Element = ();
        fn check(&mut self) -> Result<()> {
            self.checks += 1;
            if self.checks == self.fail_check {
                Err("changed")
            } else {
                Ok(())
            }
        }
        fn focused(&mut self) -> Result<()> {
            self.gets += 1;
            if self.get_error {
                Err("get_error")
            } else {
                Ok(())
            }
        }
        fn control_type(&mut self, _: &()) -> Result<i32> {
            self.types += 1;
            if self.type_error {
                Err("type_error")
            } else {
                Ok(50004)
            }
        }
        fn matches(&mut self, _: &()) -> Result<bool> {
            self.compares += 1;
            if self.compare_error {
                Err("compare_error")
            } else {
                Ok(true)
            }
        }
    }
    fn fake(fail_check: usize) -> Fake {
        Fake {
            checks: 0,
            fail_check,
            gets: 0,
            compares: 0,
            types: 0,
            type_error: false,
            get_error: false,
            compare_error: false,
        }
    }
    /// 新介入・期限・停止の検出位置によらず、見かけの一致を返さない。
    #[test]
    fn changes_before_between_and_after_calls_discard_match() {
        for (failure, gets, types, compares) in
            [(1, 0, 0, 0), (2, 1, 0, 0), (3, 1, 1, 0), (4, 1, 1, 1)]
        {
            let mut f = fake(failure);
            assert_eq!(once(&mut f), Err("changed"));
            assert_eq!((f.gets, f.types, f.compares), (gets, types, compares));
        }
        let mut f = fake(0);
        assert_eq!(
            once(&mut f),
            Ok(Sample {
                control_type: 50004,
                matched: true
            })
        );
        assert_eq!((f.gets, f.types, f.compares, f.checks), (1, 1, 1, 4));
    }
    /// API失敗時には再試行や不要な次の読み取りをしない。
    #[test]
    fn api_errors_are_not_retried() {
        let mut f = fake(0);
        f.get_error = true;
        assert_eq!(once(&mut f), Err("get_error"));
        assert_eq!((f.gets, f.compares), (1, 0));
        let mut f = fake(0);
        f.type_error = true;
        assert_eq!(once(&mut f), Err("type_error"));
        assert_eq!((f.gets, f.types, f.compares), (1, 1, 0));
        let mut f = fake(0);
        f.compare_error = true;
        assert_eq!(once(&mut f), Err("compare_error"));
        assert_eq!((f.gets, f.compares), (1, 1));
    }
}
