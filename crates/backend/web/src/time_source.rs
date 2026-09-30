//! Web `TimeSource` (`performance.now()`) and `WallClockSource` (`Date`),
//! on web-glue: one import call each, no cached function objects.

use runtime_shared::time::{TimeSource, WallClockSource};

/// Register this backend's time source with `runtime-core`.
/// Idempotent — first install wins. Should run before any
/// `debug-stats` measurement starts.
pub fn install_time_source() {
    runtime_shared::time::install_time_source(Box::new(WebTimeSource));
}

/// Register this backend's wall clock (`js Date`) with `runtime-core`.
/// Idempotent — first install wins. Required on web: the shared
/// `SystemWallClockSource` default is never installed here
/// (`SystemTime::now()` panics on wasm32-unknown-unknown), so without
/// this install `runtime_core::time::epoch_millis()` reads `0` and
/// every civil-date UI thinks it's 1970.
pub fn install_wall_clock_source() {
    runtime_shared::time::install_wall_clock_source(Box::new(WebWallClockSource));
}

/// `Date.now()` for the epoch instant; `getTimezoneOffset()` (negated —
/// JS reports UTC−local, the trait wants local−UTC) read off a fresh
/// `Date` per call so DST transitions are honored mid-session.
struct WebWallClockSource;

impl WallClockSource for WebWallClockSource {
    fn epoch_millis(&self) -> i64 {
        web_glue::dom::date_now() as i64
    }

    fn local_offset_minutes(&self) -> i32 {
        -(web_glue::dom::timezone_offset_minutes() as i32)
    }
}

/// `performance.now()` in microseconds. Holds no JS state, so the auto
/// `Send`/`Sync` the `OnceLock<Box<dyn TimeSource>>` storage needs apply
/// without an unsafe assertion.
struct WebTimeSource;

impl TimeSource for WebTimeSource {
    fn now_micros(&self) -> u64 {
        (web_glue::dom::performance_now() * 1000.0).max(0.0) as u64
    }
}
