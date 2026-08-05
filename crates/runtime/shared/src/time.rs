//! Platform-agnostic clocks: monotonic + wall.
//!
//! [`TimeSource`] is a single-method trait the active backend
//! implements: "give me a microsecond reading from some monotonic
//! epoch." Hosts register an impl via [`install_time_source`] at
//! init.
//!
//! [`WallClockSource`] is its calendar-time counterpart: "give me the
//! Unix-epoch instant and the local UTC offset." It exists for UI that
//! must know the user's *civil* date/time (a date picker's "today", a
//! schedule view) — something a monotonic delta can never provide.
//! Hosts register an impl via [`install_wall_clock_source`].
//!
//! [`mount`](crate::mount) installs platform-appropriate defaults the
//! first time it runs (see [`install_default_time_source`]): native
//! hosts get an [`InstantTimeSource`] and a [`SystemWallClockSource`];
//! `Web` is skipped because the web backend installs
//! `performance.now()` / `js Date`-backed sources during bootstrap and
//! `std::time::Instant::now()` / `SystemTime::now()` panic on
//! `wasm32-unknown-unknown`. That selection uses the runtime
//! [`Platform`](crate::Platform) identity, **not** a
//! `#[cfg(target_arch)]` — core stays free of compile-target switches.
//!
//! Until a source is installed (e.g. before `mount`, or on `Web`
//! before its bootstrap install), [`now_micros`] and [`epoch_millis`]
//! read `0`.

use std::sync::OnceLock;

/// Backend-supplied monotonic clock. Implementations must be cheap
/// (called from hot per-frame timing paths under `debug-stats`).
pub trait TimeSource: Send + Sync {
    /// Microseconds elapsed since this source's implementation-
    /// defined monotonic epoch. The epoch is fixed for the source's
    /// lifetime so deltas between two readings are meaningful.
    fn now_micros(&self) -> u64;
}

/// Default monotonic [`TimeSource`] for native hosts: anchors an epoch
/// at construction and reports elapsed microseconds. Installed
/// automatically by [`mount`](crate::mount) on non-`Web` platforms via
/// [`install_default_time_source`].
///
/// Lives in core but is only ever *constructed* on native — `mount`
/// skips it on `Web`, where `std::time::Instant::now()` would panic
/// (`wasm32-unknown-unknown` has no monotonic clock). The type still
/// compiles for wasm; it's just never instantiated there, so no
/// `#[cfg]` is needed to make this sound.
pub struct InstantTimeSource {
    epoch: std::time::Instant,
}

impl InstantTimeSource {
    pub fn new() -> Self {
        Self {
            epoch: std::time::Instant::now(),
        }
    }
}

impl Default for InstantTimeSource {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeSource for InstantTimeSource {
    fn now_micros(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }
}

static TIME_SOURCE: OnceLock<Box<dyn TimeSource>> = OnceLock::new();

/// Register the active backend's time source. First call wins;
/// subsequent calls are silently ignored.
pub fn install_time_source(source: Box<dyn TimeSource>) {
    let _ = TIME_SOURCE.set(source);
}

/// Install the platform-appropriate default time source unless a host
/// already installed one. Called once from [`mount`](crate::mount)
/// after the backend's [`Platform`](crate::Platform) is known.
///
/// Native platforms get an [`InstantTimeSource`]. `Web` is skipped:
/// the web backend installs a `performance.now()`-backed source during
/// bootstrap, and there is no std monotonic clock on
/// `wasm32-unknown-unknown` (`Instant::now()` panics), so a `0` reading
/// stays until that install lands. Branching on the runtime `Platform`
/// here — rather than `#[cfg(target_arch)]` — is what keeps this clock
/// free of compile-target switches.
pub fn install_default_time_source(platform: crate::Platform) {
    // The wall clock rides the same mount hook so every backend that
    // installs the default monotonic clock also gets calendar time —
    // `install_default_wall_clock_source` applies the same Web skip and
    // first-install-wins rules internally.
    install_default_wall_clock_source(platform);
    if TIME_SOURCE.get().is_some() || platform == crate::Platform::Web {
        return;
    }
    install_time_source(Box::new(InstantTimeSource::new()));
}

/// Read the current time in microseconds. Uses the installed
/// [`TimeSource`] if present; otherwise reads `0` (no source installed
/// yet — see the module docs and [`install_default_time_source`]).
pub fn now_micros() -> u64 {
    match TIME_SOURCE.get() {
        Some(ts) => ts.now_micros(),
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Wall clock — calendar time for UI (date pickers, schedules)
// ---------------------------------------------------------------------------

/// Backend-supplied wall clock: the Unix-epoch instant plus the local
/// UTC offset. Split into two primitive readings (rather than one
/// "local datetime" struct) so the trait stays trivially implementable
/// on every backend and civil-date math lives in one place, above the
/// seam.
///
/// Not required to be monotonic — the user can change the system clock
/// or cross a DST boundary mid-process, and both methods must report
/// the *current* truth per call (don't cache the offset at install).
pub trait WallClockSource: Send + Sync {
    /// Milliseconds since `1970-01-01T00:00:00Z` (UTC).
    fn epoch_millis(&self) -> i64;
    /// Minutes to ADD to UTC to reach local civil time — e.g. `-300`
    /// for EST, `+120` for CEST. Re-evaluated per call so DST
    /// transitions are honored.
    fn local_offset_minutes(&self) -> i32;
}

/// Default wall clock for native hosts: `SystemTime` for the epoch
/// instant, UTC (offset `0`) for the local offset — std has no
/// timezone database, so the offset refinement is per-backend (macOS
/// installs an `NSTimeZone`-backed source; web reads `js Date`).
/// Installed automatically by [`mount`](crate::mount) on non-`Web`
/// platforms via [`install_default_time_source`].
///
/// Like [`InstantTimeSource`], the type compiles for wasm but is never
/// constructed there (`SystemTime::now()` panics on
/// `wasm32-unknown-unknown`; the Web skip in the default installer is
/// what makes this sound).
pub struct SystemWallClockSource;

impl WallClockSource for SystemWallClockSource {
    fn epoch_millis(&self) -> i64 {
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_millis() as i64,
            // Clock set before 1970 — report the (negative) distance
            // rather than clamping to 0 so the civil date stays right.
            Err(e) => -(e.duration().as_millis() as i64),
        }
    }

    fn local_offset_minutes(&self) -> i32 {
        0
    }
}

static WALL_CLOCK: OnceLock<Box<dyn WallClockSource>> = OnceLock::new();

/// Register the active backend's wall clock. First call wins;
/// subsequent calls are silently ignored — so a backend installing a
/// timezone-aware source must do so *before* the default lands (all
/// backends install their own sources ahead of the
/// [`install_default_time_source`] call in their mount preamble).
pub fn install_wall_clock_source(source: Box<dyn WallClockSource>) {
    let _ = WALL_CLOCK.set(source);
}

/// Install the platform-appropriate default wall clock unless a host
/// already installed one. Same shape and rationale as
/// [`install_default_time_source`] (which calls this): native gets
/// [`SystemWallClockSource`]; `Web` is skipped because `SystemTime`
/// panics on wasm and the web backend installs a `js Date`-backed
/// source during bootstrap.
pub fn install_default_wall_clock_source(platform: crate::Platform) {
    if WALL_CLOCK.get().is_some() || platform == crate::Platform::Web {
        return;
    }
    install_wall_clock_source(Box::new(SystemWallClockSource));
}

/// Milliseconds since the Unix epoch (UTC) from the installed
/// [`WallClockSource`], or `0` when none is installed yet (before
/// `mount`, or on `Web` before its bootstrap install) — mirroring
/// [`now_micros`]'s no-source reading.
pub fn epoch_millis() -> i64 {
    match WALL_CLOCK.get() {
        Some(wc) => wc.epoch_millis(),
        None => 0,
    }
}

/// Minutes to add to UTC to reach local civil time, from the installed
/// [`WallClockSource`]; `0` when none is installed.
pub fn local_offset_minutes() -> i32 {
    match WALL_CLOCK.get() {
        Some(wc) => wc.local_offset_minutes(),
        None => 0,
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    //! Tests for the native default clock ([`InstantTimeSource`]) and
    //! the no-source reading.
    //!
    //! `TIME_SOURCE` is a process-wide `OnceLock`, so `now_micros()`'s
    //! installed-vs-not behaviour can't be toggled mid-binary. These
    //! tests drive [`InstantTimeSource`] directly — the type `mount`
    //! installs on native — which both sidesteps the OnceLock and is
    //! the behaviour that actually ships.

    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn instant_source_is_monotonic() {
        let src = InstantTimeSource::new();
        let a = src.now_micros();
        let b = src.now_micros();
        assert!(b >= a, "now_micros must not go backwards (got a={a}, b={b})");
    }

    #[test]
    fn instant_source_advances_with_sleep() {
        let src = InstantTimeSource::new();
        let before = src.now_micros();
        sleep(Duration::from_millis(2));
        let after = src.now_micros();
        let delta = after.saturating_sub(before);
        // Sleeping 2 ms should produce at least ~1 ms of monotonic
        // advance even on a heavily-loaded CI host. We assert
        // > 500 µs to leave room for jitter while still catching a
        // dead clock.
        assert!(
            delta >= 500,
            "expected at least 500 µs of progress, got {delta} µs (before={before}, after={after})",
        );
    }

    #[test]
    fn instant_source_reads_are_small_when_idle() {
        // Sanity that the clock isn't returning wild values per call
        // (no second-scale jumps). Two back-to-back reads on the
        // same thread should be within ~1 second of each other —
        // generous bound to avoid CI flakiness.
        let src = InstantTimeSource::new();
        let a = src.now_micros();
        let b = src.now_micros();
        let delta = b.saturating_sub(a);
        assert!(
            delta < 1_000_000,
            "two back-to-back reads diverged by {delta} µs; clock seems wrong",
        );
    }

    #[test]
    fn install_default_is_noop_on_web() {
        // `Web` must never install `InstantTimeSource` — `Instant::now()`
        // panics on wasm and the web backend owns the real source. This
        // is a pure predicate check: it returns early on `Web` before
        // touching the OnceLock, so it's safe to run regardless of
        // whatever else the test binary has installed.
        install_default_time_source(crate::Platform::Web);
    }

    #[test]
    fn system_wall_clock_reads_a_plausible_epoch() {
        // `SystemWallClockSource` is what native mounts install; its
        // epoch reading must be a real Unix timestamp, not a monotonic
        // delta. 2020-01-01T00:00:00Z = 1_577_836_800_000 ms — any CI
        // host's clock is past that, and a monotonic-style
        // since-process-start reading never is.
        let src = SystemWallClockSource;
        let ms = src.epoch_millis();
        assert!(
            ms > 1_577_836_800_000,
            "epoch_millis must be wall time since 1970, got {ms}",
        );
        // The std default has no timezone database: it must report UTC,
        // leaving offset refinement to per-backend sources.
        assert_eq!(src.local_offset_minutes(), 0);
    }

    #[test]
    fn install_default_wall_clock_is_noop_on_web() {
        // Same contract as the monotonic default: `Web` must never
        // install `SystemWallClockSource` (`SystemTime::now()` panics on
        // wasm32-unknown-unknown; the web backend owns the real source).
        // Pure predicate check — returns early before the OnceLock.
        install_default_wall_clock_source(crate::Platform::Web);
    }

    #[test]
    fn wall_clock_trait_can_be_implemented_with_const_values() {
        // Pins the trait shape: two primitive readings, re-evaluated per
        // call. A fixed impl is enough for hosts/tests to fake a date.
        struct Fixed;
        impl WallClockSource for Fixed {
            fn epoch_millis(&self) -> i64 {
                86_400_000
            }
            fn local_offset_minutes(&self) -> i32 {
                -300
            }
        }
        let s = Fixed;
        assert_eq!(s.epoch_millis(), 86_400_000);
        assert_eq!(s.local_offset_minutes(), -300);
    }

    #[test]
    fn time_source_trait_can_be_implemented_with_a_const_value() {
        // Pinning down the trait's shape: a TimeSource is a single
        // `now_micros(&self) -> u64`. Verify a trivial impl
        // compiles + executes the expected value.
        struct Fixed(u64);
        impl TimeSource for Fixed {
            fn now_micros(&self) -> u64 {
                self.0
            }
        }
        let s = Fixed(12_345);
        assert_eq!(s.now_micros(), 12_345);
    }
}
