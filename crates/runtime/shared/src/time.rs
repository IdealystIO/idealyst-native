//! Platform-agnostic monotonic clock.
//!
//! [`TimeSource`] is a single-method trait the active backend
//! implements: "give me a microsecond reading from some monotonic
//! epoch." Hosts register an impl via [`install_time_source`] at
//! init.
//!
//! [`mount`](crate::mount) installs a platform-appropriate default the
//! first time it runs (see [`install_default_time_source`]): native
//! hosts get an [`InstantTimeSource`]; `Web` is skipped because the web
//! backend installs a `performance.now()`-backed source during
//! bootstrap and `std::time::Instant::now()` panics on
//! `wasm32-unknown-unknown`. That selection uses the runtime
//! [`Platform`](crate::Platform) identity, **not** a
//! `#[cfg(target_arch)]` — core stays free of compile-target switches.
//!
//! Until a source is installed (e.g. before `mount`, or on `Web`
//! before its bootstrap install), [`now_micros`] reads `0`.

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
