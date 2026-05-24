//! Platform-agnostic monotonic clock.
//!
//! [`TimeSource`] is a single-method trait the active backend
//! implements: "give me a microsecond reading from some monotonic
//! epoch." Hosts register an impl via [`install_time_source`] at
//! init.
//!
//! Without an installed source:
//! - On **native** targets, falls back to `std::time::Instant`
//!   captured the first time [`now_micros`] is called.
//! - On **wasm32**, returns `0` so callers (currently only the
//!   `debug-stats` feature) keep working — all readings just read
//!   zero. Install `backend_web::install_time_source()` to get real
//!   `performance.now()` readings.

use std::sync::OnceLock;

/// Backend-supplied monotonic clock. Implementations must be cheap
/// (called from hot per-frame timing paths under `debug-stats`).
pub trait TimeSource: Send + Sync {
    /// Microseconds elapsed since this source's implementation-
    /// defined monotonic epoch. The epoch is fixed for the source's
    /// lifetime so deltas between two readings are meaningful.
    fn now_micros(&self) -> u64;
}

static TIME_SOURCE: OnceLock<Box<dyn TimeSource>> = OnceLock::new();

/// Register the active backend's time source. First call wins;
/// subsequent calls are silently ignored.
pub fn install_time_source(source: Box<dyn TimeSource>) {
    let _ = TIME_SOURCE.set(source);
}

/// Read the current time in microseconds. Uses the installed
/// [`TimeSource`] if present; otherwise the native fallback
/// (`std::time::Instant`-based) or `0` on wasm32.
pub fn now_micros() -> u64 {
    if let Some(ts) = TIME_SOURCE.get() {
        return ts.now_micros();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Anchor at process start the first time we're called so all
        // subsequent readings share one monotonic epoch.
        static NATIVE_EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
        let epoch = NATIVE_EPOCH.get_or_init(std::time::Instant::now);
        epoch.elapsed().as_micros() as u64
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    //! Native-fallback path tests for the time abstraction.
    //!
    //! `TIME_SOURCE` is a `OnceLock` so we can only install ONCE per
    //! process. The test binary's other modules don't install one,
    //! so the tests below exercise the `Instant`-based fallback (the
    //! actual path runtime-core ships on native test runs anyway).

    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn now_micros_is_monotonic_native_fallback() {
        let a = now_micros();
        let b = now_micros();
        assert!(b >= a, "now_micros must not go backwards (got a={a}, b={b})");
    }

    #[test]
    fn now_micros_advances_with_sleep() {
        let before = now_micros();
        sleep(Duration::from_millis(2));
        let after = now_micros();
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

    #[test]
    fn deltas_between_two_now_micros_calls_are_small_when_idle() {
        // Sanity that the clock isn't returning wild values per call
        // (no second-scale jumps). Two back-to-back reads on the
        // same thread should be within ~1 second of each other —
        // generous bound to avoid CI flakiness.
        let a = now_micros();
        let b = now_micros();
        let delta = b.saturating_sub(a);
        assert!(
            delta < 1_000_000,
            "two back-to-back now_micros reads diverged by {delta} µs; clock seems wrong",
        );
    }
}
