//! `set_clock` / `reset_clock` — process-wide, so alone in this test binary
//! (a parallel test reading the real clock would see the fake).

#![cfg(not(target_arch = "wasm32"))]

use datetime::{
    local_offset, local_timezone, now_utc, reset_clock, set_clock, with_clock, FixedClock,
    Timestamp, UtcOffset,
};

#[test]
fn set_clock_reaches_every_thread_and_a_scope_still_wins() {
    let fake = FixedClock::new(Timestamp::from_unix_seconds(1_000))
        .with_offset(UtcOffset::from_minutes(60).unwrap())
        .with_timezone("Europe/Paris");
    set_clock(fake);

    assert_eq!(now_utc().unix_seconds(), 1_000);
    assert_eq!(local_offset().minutes(), 60);
    assert_eq!(local_timezone().as_deref(), Some("Europe/Paris"));
    let other = std::thread::spawn(|| now_utc().unix_seconds())
        .join()
        .unwrap();
    assert_eq!(other, 1_000, "process-wide clock reaches other threads");

    with_clock(FixedClock::new(Timestamp::from_unix_seconds(7)), || {
        assert_eq!(
            now_utc().unix_seconds(),
            7,
            "a thread scope beats the global clock"
        );
    });
    assert_eq!(now_utc().unix_seconds(), 1_000);

    // Replacing swaps it.
    set_clock(FixedClock::new(Timestamp::from_unix_seconds(2_000)));
    assert_eq!(now_utc().unix_seconds(), 2_000);

    reset_clock();
    assert!(
        now_utc().unix_millis() > 1_577_836_800_000,
        "real clock after reset"
    );
}
