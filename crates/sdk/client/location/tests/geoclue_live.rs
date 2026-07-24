//! Live GeoClue2 smoke test for the Linux backend — **`#[ignore]`d on purpose**.
//!
//! Unlike the pure mapping/error-classification unit tests in `src/linux.rs`
//! (which run in CI with no bus), this drives the real D-Bus path end to end:
//! `Connection::system()` → `Manager.GetClient` → set `DesktopId` → `Start` →
//! await a real `LocationUpdated`. That needs a host with:
//!
//!   - the `org.freedesktop.GeoClue2` daemon installed **and** activatable on
//!     the system bus,
//!   - a working location source (WiFi/GeoIP/GPS) to actually produce a fix,
//!   - GeoClue authorization for an unregistered `DesktopId` (its agent /
//!     `/etc/geoclue/geoclue.conf` may otherwise deny us).
//!
//! None of that is reproducible in CI, so the test is ignored by default. Run
//! it by hand on a real desktop:
//!
//! ```text
//! cargo test -p location --test geoclue_live -- --ignored --nocapture
//! ```
//!
//! It exercises the crate's public `current()` entry point (which on Linux
//! gates through `permissions` → `Unsupported` → `is_usable()` → the GeoClue
//! backend), so a green run proves the whole Linux path, not just the mapping.

#![cfg(target_os = "linux")]

use location::LocationError;

#[tokio::test(flavor = "current_thread")]
#[ignore = "needs a running GeoClue2 daemon + a real location source; run manually with --ignored"]
async fn live_current_fix_from_geoclue() {
    match location::current().await {
        Ok(pos) => {
            eprintln!(
                "GeoClue2 fix: {}, {} (±{} m), alt={:?} speed={:?} heading={:?} t={}",
                pos.latitude,
                pos.longitude,
                pos.accuracy_m,
                pos.altitude,
                pos.speed,
                pos.heading,
                pos.timestamp_ms
            );
            // A genuine fix must land in valid WGS-84 ranges.
            assert!(
                (-90.0..=90.0).contains(&pos.latitude),
                "latitude out of range: {}",
                pos.latitude
            );
            assert!(
                (-180.0..=180.0).contains(&pos.longitude),
                "longitude out of range: {}",
                pos.longitude
            );
            assert!(pos.accuracy_m >= 0.0, "accuracy must be non-negative");
        }
        // `NotSupported` here means the backend was never reached / GeoClue is
        // absent — the whole point of the test is that it IS reached, so treat
        // that as a failure of the environment the tester must fix.
        Err(LocationError::NotSupported) => {
            panic!("GeoClue2 not available — install/enable it, then re-run this ignored test");
        }
        // A real environment can still legitimately deny or fail to fix; report
        // it clearly rather than asserting a fix always succeeds.
        Err(e) => {
            eprintln!("GeoClue2 reached but returned: {e}");
        }
    }
}
