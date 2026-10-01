//! The renderer's monotonic clock: a framework-owned [`Instant`] read
//! from `runtime_shared::time::now_micros()`.
//!
//! Why not `std::time::Instant`: `Instant::now()` panics on
//! `wasm32-unknown-unknown`, and this crate also runs embedded in a
//! web page (`host-web`). Why not the `web-time` crate (the previous
//! choice): the framework already has exactly one clock seam — the
//! backend-installed `TimeSource` (`performance.now()` on web, a
//! `std::time::Instant` epoch on native) — and a second, parallel clock
//! is a dependency for nothing. Reading the installed source keeps the
//! renderer's animation clock and every other framework timing read
//! (`PhaseTimer`, `after_ms`, …) on the same timeline.
//!
//! **Invariant: a source must be installed before the first read**, or
//! [`Instant::now`] reads `0` forever and every tween / momentum scroll
//! / caret blink freezes at its first frame. [`crate::Host::new`]
//! installs the platform default (first install wins, so a host that
//! installed its own source earlier — backend-web's `performance.now()`
//! source under `host-web` — keeps it); `newcore::start` repeats the
//! idempotent install for backends built without a `Host`.
//!
//! Durations are plain [`std::time::Duration`] — constructing and doing
//! arithmetic on one is fine on wasm32; only the `now()` reads panic.

use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::time::Duration;

/// A monotonic timestamp in microseconds on the installed
/// `runtime_shared::time` source's epoch. Mirrors the subset of
/// `std::time::Instant`'s API this crate uses, with std's saturating
/// semantics (`duration_since` of a later instant is zero, never a
/// panic).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant {
    micros: u64,
}

impl Instant {
    /// The current reading of the installed monotonic source. Reads
    /// the source's epoch (`0`) when none is installed — see the module
    /// docs for who installs it.
    #[inline]
    pub fn now() -> Self {
        Self {
            micros: runtime_shared::time::now_micros(),
        }
    }

    /// An instant at `micros` on the source's timeline. For tests and
    /// for converting a raw `now_micros()` reading.
    #[inline]
    pub const fn from_micros(micros: u64) -> Self {
        Self { micros }
    }

    /// Microseconds since the source's epoch.
    #[inline]
    pub const fn as_micros(self) -> u64 {
        self.micros
    }

    /// Time from `earlier` to `self`; zero when `earlier` is later
    /// (std's behavior since 1.60).
    #[inline]
    pub fn duration_since(self, earlier: Instant) -> Duration {
        self.saturating_duration_since(earlier)
    }

    /// Time from `earlier` to `self`, or `None` when `earlier` is later.
    #[inline]
    pub fn checked_duration_since(self, earlier: Instant) -> Option<Duration> {
        self.micros.checked_sub(earlier.micros).map(Duration::from_micros)
    }

    /// Time from `earlier` to `self`; zero when `earlier` is later.
    #[inline]
    pub fn saturating_duration_since(self, earlier: Instant) -> Duration {
        Duration::from_micros(self.micros.saturating_sub(earlier.micros))
    }

    /// Time elapsed since `self` (saturating).
    #[inline]
    pub fn elapsed(self) -> Duration {
        Instant::now().saturating_duration_since(self)
    }

    /// `self + d`, or `None` on overflow. Sub-microsecond parts of `d`
    /// are truncated (the source's resolution is 1 µs).
    #[inline]
    pub fn checked_add(self, d: Duration) -> Option<Instant> {
        let d = u64::try_from(d.as_micros()).ok()?;
        self.micros.checked_add(d).map(Instant::from_micros)
    }

    /// `self - d`, or `None` if it would precede the epoch.
    #[inline]
    pub fn checked_sub(self, d: Duration) -> Option<Instant> {
        let d = u64::try_from(d.as_micros()).ok()?;
        self.micros.checked_sub(d).map(Instant::from_micros)
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;
    /// Panics on overflow, like `std::time::Instant`.
    fn add(self, d: Duration) -> Instant {
        self.checked_add(d)
            .expect("overflow when adding duration to instant")
    }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, d: Duration) {
        *self = *self + d;
    }
}

impl Sub<Duration> for Instant {
    type Output = Instant;
    /// Panics on underflow, like `std::time::Instant`.
    fn sub(self, d: Duration) -> Instant {
        self.checked_sub(d)
            .expect("overflow when subtracting duration from instant")
    }
}

impl SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, d: Duration) {
        *self = *self - d;
    }
}

impl Sub<Instant> for Instant {
    type Output = Duration;
    /// Saturating, like `std::time::Instant` since 1.60.
    fn sub(self, earlier: Instant) -> Duration {
        self.saturating_duration_since(earlier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_and_differences_match_std_semantics() {
        let a = Instant::from_micros(1_000);
        let b = Instant::from_micros(3_500);
        assert!(a < b);
        assert_eq!(b.duration_since(a), Duration::from_micros(2_500));
        assert_eq!(b - a, Duration::from_micros(2_500));
        // Saturating in the "wrong" direction, never a panic.
        assert_eq!(a.duration_since(b), Duration::ZERO);
        assert_eq!(a.saturating_duration_since(b), Duration::ZERO);
        assert_eq!(a - b, Duration::ZERO);
        assert_eq!(a.checked_duration_since(b), None);
        assert_eq!(b.checked_duration_since(a), Some(Duration::from_micros(2_500)));
    }

    #[test]
    fn duration_arithmetic_round_trips() {
        let a = Instant::from_micros(10_000);
        let later = a + Duration::from_millis(5);
        assert_eq!(later.as_micros(), 15_000);
        assert_eq!(later - Duration::from_millis(5), a);
        let mut c = a;
        c += Duration::from_micros(7);
        c -= Duration::from_micros(2);
        assert_eq!(c.as_micros(), 10_005);
        assert_eq!(a.checked_sub(Duration::from_millis(11)), None);
        assert_eq!(Instant::from_micros(u64::MAX).checked_add(Duration::from_micros(1)), None);
        // Sub-µs precision truncates (source resolution is 1 µs).
        assert_eq!((a + Duration::from_nanos(1_999)).as_micros(), 10_001);
    }

    /// The failure mode this module guards: with no installed source
    /// `now()` reads 0 forever and every animation freezes. With the
    /// native default installed, the clock must actually advance and
    /// `elapsed` must see it.
    #[test]
    fn regression_instant_advances_with_installed_source() {
        runtime_shared::time::install_default_time_source(runtime_shared::Platform::MacOs);
        let start = Instant::now();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut later = Instant::now();
        while later <= start {
            assert!(std::time::Instant::now() < deadline, "renderer clock is frozen");
            std::thread::sleep(Duration::from_millis(1));
            later = Instant::now();
        }
        assert!(later.duration_since(start) > Duration::ZERO);
        std::thread::sleep(Duration::from_millis(5));
        assert!(start.elapsed() >= Duration::from_millis(5));
    }
}
