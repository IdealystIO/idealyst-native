//! The current **date and time** on every target: the wall-clock instant,
//! the local UTC offset at any instant, and the local IANA zone name.
//!
//! ```
//! let now = datetime::now_utc();                 // Timestamp, µs since 1970-01-01T00:00:00Z
//! let ms = now.unix_millis();
//! let offset = datetime::local_offset_at(now);   // UtcOffset, DST-correct for that instant
//! let zone = datetime::local_timezone();         // Some("Europe/Berlin") when known
//! # let _ = (ms, offset, zone);
//! ```
//!
//! With the `chrono` feature, one call replaces each chrono clock read:
//!
//! | chrono (needs `clock`, and `wasmbind` on web) | this crate |
//! | --- | --- |
//! | `Utc::now()` | [`now_chrono_utc()`] → `DateTime<Utc>` |
//! | `Local::now()` | [`now_chrono_local()`] → `DateTime<FixedOffset>` |
//! | `Local::now().date_naive()` / `Local::today()` | `now_chrono_local().date_naive()` |
//! | `dt.with_timezone(&Local)` | [`to_chrono_local`]`(dt.into())` |
//!
//! # Why this exists
//!
//! chrono's `Utc::now()` / `Local::now()` reach the browser only through its
//! `wasmbind` feature, which links `wasm-bindgen` + `js-sys` into the app and
//! forces the slower hybrid web build. Without `wasmbind` they fall back to
//! `std::time::SystemTime`, which **panics** on `wasm32-unknown-unknown`.
//! This crate reads the browser's `Date` through web-glue instead, and the
//! platform clock everywhere else, so the same call works on every target and
//! keeps an app's web build in own mode.
//!
//! # Scope
//!
//! Reading the clock and the local zone, nothing more. Calendar arithmetic,
//! parsing and formatting stay with chrono (or whichever date library the app
//! uses) — the `chrono` feature hands over chrono values, and [`Timestamp`]
//! converts to and from plain Unix epoch numbers for anything else.
//!
//! # Fake time in tests
//!
//! Every read goes through a [`Clock`]. By default it is [`SystemClock`];
//! [`with_clock`] swaps in another one (usually a [`FixedClock`]) for the
//! current thread for the duration of a closure, and [`set_clock`] swaps it
//! process-wide until [`reset_clock`]:
//!
//! ```
//! use datetime::{with_clock, FixedClock, Timestamp, UtcOffset};
//!
//! let clock = FixedClock::new(Timestamp::from_unix_millis(1_700_000_000_000))
//!     .with_offset(UtcOffset::from_minutes(-300).unwrap())
//!     .with_timezone("America/New_York");
//! with_clock(clock, || {
//!     assert_eq!(datetime::now_utc().unix_millis(), 1_700_000_000_000);
//!     assert_eq!(datetime::local_offset().minutes(), -300);
//! });
//! ```

#![deny(missing_docs)]

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

// Exactly one platform clock per target. Both expose the same three fns:
// `now_micros() -> i64`, `offset_seconds_at(unix_micros: i64) -> i32` and
// `timezone() -> Option<String>`.
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
use native as platform;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
use web as platform;

#[cfg(feature = "chrono")]
mod chrono_support;
#[cfg(feature = "chrono")]
pub use chrono_support::{now_chrono_local, now_chrono_utc, to_chrono_local};

const MICROS_PER_MILLI: i64 = 1_000;
const MICROS_PER_SECOND: i64 = 1_000_000;
const SECONDS_PER_MINUTE: i32 = 60;
/// Exclusive bound on an offset's magnitude: one day. Real zones stay within
/// ±14 h; the bound is chrono's `FixedOffset` range, so every [`UtcOffset`]
/// converts to one.
const MAX_OFFSET_SECONDS: i32 = 86_400;

// ---------------------------------------------------------------------------
// Timestamp
// ---------------------------------------------------------------------------

/// An instant in time: microseconds since `1970-01-01T00:00:00Z`.
///
/// Carries no zone — it is the same instant everywhere. Negative values are
/// instants before 1970. `i64` microseconds spans ±292,000 years.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    micros: i64,
}

impl Timestamp {
    /// `1970-01-01T00:00:00Z`.
    pub const UNIX_EPOCH: Timestamp = Timestamp { micros: 0 };

    /// The instant `micros` microseconds after the Unix epoch.
    pub const fn from_unix_micros(micros: i64) -> Timestamp {
        Timestamp { micros }
    }

    /// The instant `millis` milliseconds after the Unix epoch (saturating at
    /// the representable range).
    pub const fn from_unix_millis(millis: i64) -> Timestamp {
        Timestamp {
            micros: millis.saturating_mul(MICROS_PER_MILLI),
        }
    }

    /// The instant `seconds` seconds after the Unix epoch (saturating at the
    /// representable range).
    pub const fn from_unix_seconds(seconds: i64) -> Timestamp {
        Timestamp {
            micros: seconds.saturating_mul(MICROS_PER_SECOND),
        }
    }

    /// Microseconds since the Unix epoch.
    pub const fn unix_micros(self) -> i64 {
        self.micros
    }

    /// Whole milliseconds since the Unix epoch, rounded toward the past (so
    /// an instant before 1970 never rounds up into a later millisecond).
    pub const fn unix_millis(self) -> i64 {
        self.micros.div_euclid(MICROS_PER_MILLI)
    }

    /// Whole seconds since the Unix epoch, rounded toward the past.
    pub const fn unix_seconds(self) -> i64 {
        self.micros.div_euclid(MICROS_PER_SECOND)
    }

    /// The signed distance `self - earlier` in microseconds (saturating).
    pub const fn micros_since(self, earlier: Timestamp) -> i64 {
        self.micros.saturating_sub(earlier.micros)
    }
}

// ---------------------------------------------------------------------------
// UtcOffset
// ---------------------------------------------------------------------------

/// A local zone's distance from UTC: seconds to **add** to UTC to get local
/// civil time (`+3600` for CET, `-18000` for EST). Always within ±24 h.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcOffset {
    seconds: i32,
}

impl UtcOffset {
    /// UTC itself (offset zero).
    pub const UTC: UtcOffset = UtcOffset { seconds: 0 };

    /// An offset of `seconds` east of UTC, or `None` outside ±24 h.
    pub const fn from_seconds(seconds: i32) -> Option<UtcOffset> {
        if seconds > -MAX_OFFSET_SECONDS && seconds < MAX_OFFSET_SECONDS {
            Some(UtcOffset { seconds })
        } else {
            None
        }
    }

    /// An offset of `minutes` east of UTC, or `None` outside ±24 h.
    pub const fn from_minutes(minutes: i32) -> Option<UtcOffset> {
        match minutes.checked_mul(SECONDS_PER_MINUTE) {
            Some(s) => UtcOffset::from_seconds(s),
            None => None,
        }
    }

    /// Seconds to add to UTC to get local time.
    pub const fn seconds(self) -> i32 {
        self.seconds
    }

    /// Whole minutes to add to UTC to get local time (truncated toward zero;
    /// every zone in use today is a whole number of minutes).
    pub const fn minutes(self) -> i32 {
        self.seconds / SECONDS_PER_MINUTE
    }

    /// Clamp a platform reading into range. Platforms never report more than
    /// ±14 h; this only guards the type's invariant against a garbage value.
    pub(crate) fn from_platform_seconds(seconds: i32) -> UtcOffset {
        UtcOffset::from_seconds(seconds).unwrap_or(UtcOffset::UTC)
    }
}

impl fmt::Display for UtcOffset {
    /// `+HH:MM` (or `+HH:MM:SS` for a sub-minute offset), e.g. `-05:00`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.seconds < 0 { '-' } else { '+' };
        let abs = self.seconds.unsigned_abs();
        let (h, m, s) = (abs / 3600, (abs / 60) % 60, abs % 60);
        if s == 0 {
            write!(f, "{sign}{h:02}:{m:02}")
        } else {
            write!(f, "{sign}{h:02}:{m:02}:{s:02}")
        }
    }
}

// ---------------------------------------------------------------------------
// Clock
// ---------------------------------------------------------------------------

/// Where every read in this crate comes from. [`SystemClock`] is the real
/// one; implement this (or use [`FixedClock`]) to fake time.
///
/// `Send + Sync` because [`set_clock`] shares one clock across threads.
pub trait Clock: Send + Sync {
    /// The current instant.
    fn now(&self) -> Timestamp;
    /// The local zone's UTC offset at `at` (past, present or future — so a
    /// DST transition between `at` and now is honored).
    fn local_offset_at(&self, at: Timestamp) -> UtcOffset;
    /// The local IANA zone name (`"America/New_York"`), or `None` when the
    /// platform doesn't say.
    fn local_timezone(&self) -> Option<String>;
}

/// The platform's clock and local zone. Reads fresh on every call: a system
/// clock change, a DST transition or the user picking another zone mid-run
/// shows up on the next read.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_micros(platform::now_micros())
    }

    fn local_offset_at(&self, at: Timestamp) -> UtcOffset {
        UtcOffset::from_platform_seconds(platform::offset_seconds_at(at.unix_micros()))
    }

    fn local_timezone(&self) -> Option<String> {
        platform::timezone()
    }
}

/// A clock stopped at one instant, in a zone with one fixed offset (UTC and
/// no zone name unless set). For tests and screenshots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixedClock {
    now: Timestamp,
    offset: UtcOffset,
    timezone: Option<String>,
}

impl FixedClock {
    /// Stopped at `now`, offset UTC, no zone name.
    pub fn new(now: Timestamp) -> FixedClock {
        FixedClock {
            now,
            offset: UtcOffset::UTC,
            timezone: None,
        }
    }

    /// Report `offset` for every instant.
    pub fn with_offset(mut self, offset: UtcOffset) -> FixedClock {
        self.offset = offset;
        self
    }

    /// Report `name` from [`local_timezone`].
    pub fn with_timezone(mut self, name: impl Into<String>) -> FixedClock {
        self.timezone = Some(name.into());
        self
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.now
    }

    fn local_offset_at(&self, _at: Timestamp) -> UtcOffset {
        self.offset
    }

    fn local_timezone(&self) -> Option<String> {
        self.timezone.clone()
    }
}

// ---------------------------------------------------------------------------
// Clock selection: thread-scoped override > process-wide override > system
// ---------------------------------------------------------------------------

// Two override scopes because tests and apps need different things. Rust
// runs a binary's tests on parallel threads, so a test's fake clock must not
// leak into its neighbours: `with_clock` is per thread. A whole-app fake (a
// screenshot run, an E2E fixture) must reach every thread and task:
// `set_clock` is process-wide. The atomic keeps the unset case (every
// production read) to one relaxed load, no lock.
static GLOBAL_SET: AtomicBool = AtomicBool::new(false);
static GLOBAL: RwLock<Option<Arc<dyn Clock>>> = RwLock::new(None);

thread_local! {
    static SCOPED: RefCell<Vec<Arc<dyn Clock>>> = const { RefCell::new(Vec::new()) };
}

/// Replace the clock for the whole process until [`reset_clock`]. A
/// [`with_clock`] scope on a thread still wins over it on that thread.
pub fn set_clock(clock: impl Clock + 'static) {
    let mut slot = GLOBAL.write().unwrap_or_else(|e| e.into_inner());
    *slot = Some(Arc::new(clock));
    GLOBAL_SET.store(true, Ordering::Release);
}

/// Undo [`set_clock`]: reads go back to [`SystemClock`].
pub fn reset_clock() {
    let mut slot = GLOBAL.write().unwrap_or_else(|e| e.into_inner());
    *slot = None;
    GLOBAL_SET.store(false, Ordering::Release);
}

/// Run `f` with `clock` as the clock for **this thread**, then restore the
/// previous one (also when `f` panics). Scopes nest; the innermost wins.
pub fn with_clock<R>(clock: impl Clock + 'static, f: impl FnOnce() -> R) -> R {
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            SCOPED.with(|s| {
                s.borrow_mut().pop();
            });
        }
    }
    SCOPED.with(|s| s.borrow_mut().push(Arc::new(clock)));
    let _pop = Pop;
    f()
}

/// Run `read` against the clock in effect on this thread.
fn read<R>(read: impl Fn(&dyn Clock) -> R) -> R {
    // Clone the Arc out rather than holding the RefCell borrow across the
    // read: a custom `Clock` impl may itself call this crate.
    if let Some(clock) = SCOPED.with(|s| s.borrow().last().cloned()) {
        return read(&*clock);
    }
    if GLOBAL_SET.load(Ordering::Acquire) {
        let clock = GLOBAL.read().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(clock) = clock {
            return read(&*clock);
        }
    }
    read(&SystemClock)
}

// ---------------------------------------------------------------------------
// The reads
// ---------------------------------------------------------------------------

/// The current instant (UTC). Microsecond resolution on native; the browser
/// reports whole milliseconds.
pub fn now_utc() -> Timestamp {
    read(|c| c.now())
}

/// The local zone's UTC offset at `at`, with that instant's DST rule — a
/// July instant in New York gives `-04:00`, a January one `-05:00`, whatever
/// today's date is.
///
/// Where the platform has no zone information (a bare `wasm32-wasi`, an
/// unknown OS) this is UTC.
pub fn local_offset_at(at: Timestamp) -> UtcOffset {
    read(|c| c.local_offset_at(at))
}

/// The local zone's UTC offset right now. Reads the clock once; use
/// [`local_offset_at`] to pair an offset with an instant you already hold.
pub fn local_offset() -> UtcOffset {
    read(|c| c.local_offset_at(c.now()))
}

/// The local IANA zone name (`"Australia/Sydney"`, `"UTC"`), or `None` when
/// the platform doesn't report one.
///
/// Web: `Intl.DateTimeFormat().resolvedOptions().timeZone`. Native: `$TZ`
/// when it names an IANA zone (`$TZ` set to a POSIX rule such as
/// `EST5EDT,M3.2.0,M11.1.0` gives `None`, since no zone name describes it),
/// else the system setting (CFTimeZone on Apple, the
/// `persist.sys.timezone` property on Android, `/etc/localtime` on Linux,
/// the WinRT calendar on Windows).
pub fn local_timezone() -> Option<String> {
    read(|c| c.local_timezone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_millis_rounds_toward_the_past_before_1970() {
        // -1 µs is 1969-12-31T23:59:59.999999Z: millisecond -1, second -1.
        let t = Timestamp::from_unix_micros(-1);
        assert_eq!(t.unix_millis(), -1);
        assert_eq!(t.unix_seconds(), -1);
        let t = Timestamp::from_unix_micros(1_999);
        assert_eq!(t.unix_millis(), 1);
        assert_eq!(t.unix_seconds(), 0);
    }

    #[test]
    fn constructors_saturate_instead_of_overflowing() {
        assert_eq!(
            Timestamp::from_unix_seconds(i64::MAX).unix_micros(),
            i64::MAX
        );
        assert_eq!(
            Timestamp::from_unix_millis(i64::MIN).unix_micros(),
            i64::MIN
        );
    }

    #[test]
    fn offset_range_and_display() {
        assert_eq!(UtcOffset::from_seconds(86_400), None);
        assert_eq!(UtcOffset::from_seconds(-86_400), None);
        assert_eq!(UtcOffset::from_minutes(i32::MAX), None);
        assert_eq!(UtcOffset::from_minutes(330).unwrap().to_string(), "+05:30");
        assert_eq!(UtcOffset::from_minutes(-300).unwrap().to_string(), "-05:00");
        assert_eq!(
            UtcOffset::from_seconds(-37).unwrap().to_string(),
            "-00:00:37"
        );
        assert_eq!(UtcOffset::UTC.to_string(), "+00:00");
        assert_eq!(UtcOffset::from_platform_seconds(1_000_000), UtcOffset::UTC);
    }
}
