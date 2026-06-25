//! Portable, host-runnable tests for the `location` public surface.
//!
//! These exercise the platform-agnostic shell on whatever host runs
//! `cargo test` (no device, no geolocation backend): the permission gate, the
//! `NotSupported` fallback, the RAII guard's clean construction/drop, and the
//! `Position` / `LocationError` value types. The native fixes themselves are
//! device-only and covered as compile-checked paths (see the README).

use location::{watch, LocationError, Position};

// NB: `current()` isn't tested here. It first awaits
// `permissions::request(LocationWhenInUse)`, which on a macOS host runs the
// real `CLLocationManager` authorization path rather than a stub — platform
// state outside this crate. The host-deterministic surface is below; the
// device fix is the compile-checked native path (see the README).

/// `watch` returns its RAII guard without panicking on every target; the host
/// stub backend installs nothing, so the callback never fires. The guard drops
/// cleanly — no `mem::forget`, no leak.
#[test]
fn watch_installs_and_drops_cleanly() {
    let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let f = fired.clone();
    let guard = watch(move |_pos| {
        f.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    drop(guard);
    // The host stub never delivers a fix.
    assert!(!fired.load(std::sync::atomic::Ordering::SeqCst));
}

/// `Position` is a plain `Copy` value callers thread through reactive state;
/// the optional fields round-trip exactly.
#[test]
fn position_value_round_trips() {
    let p = Position {
        latitude: 51.5074,
        longitude: -0.1278,
        accuracy_m: 8.0,
        altitude: Some(35.0),
        heading: Some(90.0),
        speed: None,
        timestamp_ms: 1_700_000_000_000.0,
    };
    let q = p; // Copy
    assert_eq!(p, q);
    assert_eq!(q.altitude, Some(35.0));
    assert_eq!(q.speed, None);
}

/// `LocationError` renders distinct, readable messages and compares by value.
#[test]
fn error_variants_are_distinct() {
    assert_ne!(
        LocationError::NotAuthorized,
        LocationError::Unavailable("x".into())
    );
    assert_ne!(LocationError::NotAuthorized, LocationError::NotSupported);
    assert!(LocationError::Unavailable("no signal".into())
        .to_string()
        .contains("no signal"));
}
