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
