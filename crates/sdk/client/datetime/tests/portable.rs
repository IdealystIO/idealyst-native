//! Target-independent behavior: the real clock is plausible, a scoped fake
//! clock is honored and restored, and (with `chrono`) the chrono values agree
//! with the crate's own readings.
//!
//! Process-wide `set_clock` lives in `tests/global_clock.rs`, its own test
//! binary, so it can't leak into the parallel tests here.

#![cfg(not(target_arch = "wasm32"))]

use datetime::{
    local_offset, local_offset_at, local_timezone, now_utc, with_clock, Clock, FixedClock,
    SystemClock, Timestamp, UtcOffset,
};

/// 2020-01-01T00:00:00Z. Any real clock is past it; a monotonic
/// since-boot reading never is.
const Y2020_MS: i64 = 1_577_836_800_000;
/// 2023-11-14T22:13:20Z.
const FIXED_MS: i64 = 1_700_000_000_000;

#[test]
fn system_clock_reads_wall_time_close_to_std() {
    let std_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let ours = now_utc().unix_millis();
    assert!(
        ours > Y2020_MS,
        "now_utc must be Unix wall time, got {ours}"
    );
    assert!(
        (ours - std_ms).abs() < 5_000,
        "now_utc {ours} vs SystemTime {std_ms}"
    );
}

#[test]
fn system_clock_offset_is_a_whole_number_of_minutes_in_range() {
    let o = SystemClock.local_offset_at(now_utc());
    assert!(o.seconds().abs() <= 14 * 3_600, "{o}");
    assert_eq!(o.seconds() % 60, 0, "{o}");
}

#[test]
fn with_clock_fakes_every_read_and_restores_after() {
    let fake = FixedClock::new(Timestamp::from_unix_millis(FIXED_MS))
        .with_offset(UtcOffset::from_minutes(330).unwrap())
        .with_timezone("Asia/Kolkata");
    with_clock(fake, || {
        assert_eq!(now_utc().unix_millis(), FIXED_MS);
        assert_eq!(local_offset().minutes(), 330);
        assert_eq!(local_offset_at(Timestamp::UNIX_EPOCH).minutes(), 330);
        assert_eq!(local_timezone().as_deref(), Some("Asia/Kolkata"));
    });
    assert!(
        now_utc().unix_millis() > Y2020_MS + 1,
        "real clock back after the scope"
    );
    assert_ne!(now_utc().unix_millis(), FIXED_MS);
}

#[test]
fn with_clock_scopes_nest_and_unwind_on_panic() {
    let outer = FixedClock::new(Timestamp::from_unix_seconds(10));
    let inner = FixedClock::new(Timestamp::from_unix_seconds(20));
    with_clock(outer, || {
        assert_eq!(now_utc().unix_seconds(), 10);
        let caught = std::panic::catch_unwind(|| {
            with_clock(inner, || {
                assert_eq!(now_utc().unix_seconds(), 20);
                panic!("unwind through the scope");
            })
        });
        assert!(caught.is_err());
        // The inner scope was popped by unwinding, not left on the stack.
        assert_eq!(now_utc().unix_seconds(), 10);
    });
}

#[test]
fn with_clock_is_per_thread() {
    with_clock(FixedClock::new(Timestamp::UNIX_EPOCH), || {
        let other = std::thread::spawn(|| now_utc().unix_millis())
            .join()
            .unwrap();
        assert!(
            other > Y2020_MS,
            "another thread must still see the real clock"
        );
        assert_eq!(now_utc(), Timestamp::UNIX_EPOCH);
    });
}

#[test]
fn a_custom_clock_can_follow_an_offset_rule() {
    // A hand-written `Clock` whose offset depends on the instant — the
    // shape an app uses to fake a DST transition.
    struct Switching;
    impl Clock for Switching {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_seconds(100)
        }
        fn local_offset_at(&self, at: Timestamp) -> UtcOffset {
            let minutes = if at.unix_seconds() < 50 { 60 } else { 120 };
            UtcOffset::from_minutes(minutes).unwrap()
        }
        fn local_timezone(&self) -> Option<String> {
            None
        }
    }
    with_clock(Switching, || {
        assert_eq!(
            local_offset_at(Timestamp::from_unix_seconds(0)).minutes(),
            60
        );
        assert_eq!(local_offset().minutes(), 120);
        assert_eq!(local_timezone(), None);
    });
}

#[cfg(feature = "chrono")]
mod chrono_conversions {
    use super::*;
    use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, TimeZone, Timelike, Utc};
    use datetime::{now_chrono_local, now_chrono_utc, to_chrono_local};

    #[test]
    fn now_chrono_utc_is_the_fixed_instant() {
        with_clock(
            FixedClock::new(Timestamp::from_unix_millis(FIXED_MS)),
            || {
                let now = now_chrono_utc();
                assert_eq!(now.timestamp_millis(), FIXED_MS);
                assert_eq!(now, Utc.with_ymd_and_hms(2023, 11, 14, 22, 13, 20).unwrap());
            },
        );
    }

    #[test]
    fn now_chrono_local_applies_the_offset() {
        // 22:13:20Z at -05:00 is 17:13:20 on the same date; at +05:30 it
        // is 03:43:20 the NEXT day — the local date differs from UTC's.
        let clock = |m| {
            FixedClock::new(Timestamp::from_unix_millis(FIXED_MS))
                .with_offset(UtcOffset::from_minutes(m).unwrap())
        };
        with_clock(clock(-300), || {
            let local = now_chrono_local();
            let wall = NaiveDate::from_ymd_opt(2023, 11, 14)
                .unwrap()
                .and_hms_opt(17, 13, 20)
                .unwrap();
            assert_eq!(local.naive_local(), wall);
            assert_eq!(local.offset().local_minus_utc(), -300 * 60);
            assert_eq!(
                local.date_naive(),
                NaiveDate::from_ymd_opt(2023, 11, 14).unwrap()
            );
            assert_eq!(local.timestamp_millis(), FIXED_MS);
        });
        with_clock(clock(330), || {
            let local = now_chrono_local();
            assert_eq!(local.date_naive().day(), 15);
            assert_eq!((local.hour(), local.minute()), (3, 43));
            assert_eq!(local.offset().local_minus_utc(), 330 * 60);
        });
    }

    #[test]
    fn to_chrono_local_uses_the_offset_at_that_instant() {
        struct Rule;
        impl Clock for Rule {
            fn now(&self) -> Timestamp {
                Timestamp::UNIX_EPOCH
            }
            fn local_offset_at(&self, at: Timestamp) -> UtcOffset {
                UtcOffset::from_minutes(if at.unix_seconds() < 1_000 {
                    -300
                } else {
                    -240
                })
                .unwrap()
            }
            fn local_timezone(&self) -> Option<String> {
                None
            }
        }
        with_clock(Rule, || {
            assert_eq!(
                to_chrono_local(Timestamp::from_unix_seconds(0))
                    .offset()
                    .local_minus_utc(),
                -300 * 60
            );
            assert_eq!(
                to_chrono_local(Timestamp::from_unix_seconds(5_000))
                    .offset()
                    .local_minus_utc(),
                -240 * 60
            );
        });
    }

    #[test]
    fn timestamp_round_trips_through_chrono() {
        for micros in [
            0,
            1,
            -1,
            FIXED_MS * 1_000 + 123_456,
            -62_135_596_800_000_000,
        ] {
            let t = Timestamp::from_unix_micros(micros);
            let dt: DateTime<Utc> = t.into();
            assert_eq!(Timestamp::from(dt), t);
            let fixed = dt.with_timezone(&FixedOffset::east_opt(-7 * 3_600).unwrap());
            assert_eq!(
                Timestamp::from(fixed),
                t,
                "zone does not change the instant"
            );
        }
    }

    #[test]
    fn out_of_chrono_range_saturates() {
        let max: DateTime<Utc> = Timestamp::from_unix_micros(i64::MAX).into();
        let min: DateTime<Utc> = Timestamp::from_unix_micros(i64::MIN).into();
        assert_eq!(max, DateTime::<Utc>::MAX_UTC);
        assert_eq!(min, DateTime::<Utc>::MIN_UTC);
    }

    #[test]
    fn offsets_convert_both_ways() {
        let o = UtcOffset::from_minutes(-570).unwrap();
        let f: FixedOffset = o.into();
        assert_eq!(f.local_minus_utc(), -570 * 60);
        assert_eq!(UtcOffset::from(f), o);
    }
}
