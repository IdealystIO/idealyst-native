//! The wall-clock fake a test sets (`test-clock`, driven by the robot's
//! `clock.*` functions) is what this SDK's `SystemClock` reports. Its own
//! binary: the fake is process-wide and would move the other tests' "now".

/// want_0a1bd568 (CrewForge): the kiosk reads `now_chrono_utc()` for its
/// time rules (a late check-in, a night shift rolling over). A robot clock
/// that only reached the framework's clock left the kiosk on real time.
#[test]
fn regression_now_reports_the_test_clock_fake() {
    // 2026-11-17T22:30:00Z, frozen.
    let at_ms = 1_794_954_600_000_i64;
    let real = datetime::now_utc().unix_micros();
    test_clock::set(at_ms * 1000, real, true);
    assert_eq!(datetime::now_utc().unix_micros(), at_ms * 1000);

    test_clock::advance(2 * 3600 * 1_000_000, real);
    assert_eq!(datetime::now_utc().unix_micros(), (at_ms + 7_200_000) * 1000, "two hours on");

    test_clock::set_zone(Some(-7 * 3600));
    assert_eq!(datetime::local_offset().to_string(), "-07:00");

    test_clock::clear();
    let back = datetime::now_utc().unix_micros();
    assert!((back - real).abs() < 60_000_000, "the real clock again");
}
