//! A test's hand on the wall clock. See the crate's `Cargo.toml` for why
//! this sits below both clocks.
//!
//! While a fake is set, [`apply_micros`] turns the real instant into the
//! faked one: the instant set, moving on with real time from there unless
//! frozen. [`apply_offset_seconds`] does the same for the local zone.
//! Process-wide: a clock is read from any thread.
//!
//! Only the wall clock. Monotonic time, and the timers built on it, keep
//! real time.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

struct Fake {
    /// The faked instant (µs since 1970 UTC) at `real_at`.
    at: i64,
    /// The real instant when `at` was set: a running fake moves on from here.
    real_at: i64,
    frozen: bool,
}

/// Set while a fake instant or zone is in force: the unset case (every
/// read outside a test) is one relaxed load, no lock.
static ACTIVE: AtomicBool = AtomicBool::new(false);
static FAKE: Mutex<Option<Fake>> = Mutex::new(None);
/// Faked local offset, seconds east of UTC.
static ZONE: Mutex<Option<i32>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn refresh_active() {
    let on = lock(&FAKE).is_some() || lock(&ZONE).is_some();
    ACTIVE.store(on, Ordering::Release);
}

/// The instant a clock should report, given the real one (µs since 1970).
#[inline]
pub fn apply_micros(real: i64) -> i64 {
    if !ACTIVE.load(Ordering::Acquire) {
        return real;
    }
    match &*lock(&FAKE) {
        Some(f) if f.frozen => f.at,
        Some(f) => f.at + (real - f.real_at),
        None => real,
    }
}

/// The local offset a clock should report (seconds east of UTC), given
/// the real one.
#[inline]
pub fn apply_offset_seconds(real: i32) -> i32 {
    if !ACTIVE.load(Ordering::Acquire) {
        return real;
    }
    lock(&ZONE).unwrap_or(real)
}

/// Is a fake zone set? A clock that resolves the offset at other instants
/// (DST rules) reports the fake one for every instant while it is.
pub fn zone_is_faked() -> bool {
    ACTIVE.load(Ordering::Acquire) && lock(&ZONE).is_some()
}

/// The clock reads `at` (µs) now, given the real instant `real`, and runs
/// on from there unless `frozen`.
pub fn set(at: i64, real: i64, frozen: bool) {
    *lock(&FAKE) = Some(Fake { at, real_at: real, frozen });
    refresh_active();
}

/// Move the clock `by` µs (negative: back) from what it reads now; a
/// frozen clock stays frozen.
pub fn advance(by: i64, real: i64) {
    let mut fake = lock(&FAKE);
    let (now, frozen) = match &*fake {
        Some(f) if f.frozen => (f.at, true),
        Some(f) => (f.at + (real - f.real_at), false),
        None => (real, false),
    };
    *fake = Some(Fake { at: now + by, real_at: real, frozen });
    drop(fake);
    refresh_active();
}

/// Stop the clock where it reads now (`false`: let it run on from there).
pub fn freeze(frozen: bool, real: i64) {
    let now = {
        let fake = lock(&FAKE);
        match &*fake {
            Some(f) if f.frozen => f.at,
            Some(f) => f.at + (real - f.real_at),
            None => real,
        }
    };
    set(now, real, frozen);
}

/// Report `offset_seconds` east of UTC as the local zone (`None`: the real
/// one).
pub fn set_zone(offset_seconds: Option<i32>) {
    *lock(&ZONE) = offset_seconds;
    refresh_active();
}

/// Back to the real clock and zone.
pub fn clear() {
    *lock(&FAKE) = None;
    *lock(&ZONE) = None;
    refresh_active();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fake is process-wide; tests that set it take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn unset_reads_through() {
        let _s = lock(&SERIAL);
        clear();
        assert_eq!(apply_micros(42), 42);
        assert_eq!(apply_offset_seconds(3600), 3600);
    }

    #[test]
    fn a_frozen_fake_holds_and_advances_by_hand() {
        let _s = lock(&SERIAL);
        set(1_000_000, 10, true);
        assert_eq!(apply_micros(10), 1_000_000);
        assert_eq!(apply_micros(9_999_999), 1_000_000, "real time does not move it");
        advance(90, 77);
        assert_eq!(apply_micros(0), 1_000_090);
        clear();
        assert_eq!(apply_micros(5), 5);
    }

    #[test]
    fn a_running_fake_moves_with_real_time() {
        let _s = lock(&SERIAL);
        set(500, 100, false);
        assert_eq!(apply_micros(160), 560);
        freeze(true, 200);
        assert_eq!(apply_micros(10_000), 600, "frozen where it read");
        clear();
    }

    #[test]
    fn the_zone_is_faked_on_its_own() {
        let _s = lock(&SERIAL);
        set_zone(Some(-7 * 3600));
        assert!(zone_is_faked());
        assert_eq!(apply_offset_seconds(0), -25_200);
        assert_eq!(apply_micros(9), 9, "the instant is untouched");
        clear();
        assert!(!zone_is_faked());
    }
}
