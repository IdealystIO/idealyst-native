//! Cross-platform **device geolocation** — the device's raw position, as a
//! one-shot fix or a continuous stream of updates.
//!
//! The smallest useful abstraction over each platform's location stack:
//! [`current`] requests permission (through the `permissions` SDK) and
//! resolves a single [`Position`]; [`watch`] streams positions into a
//! callback until the returned [`LocationWatch`] guard is dropped. No
//! geocoding, no map rendering, no reactive `Signal` bindings, no background
//! tracking — those are deliberately separate, higher-level layers. This
//! crate just establishes the position feed.
//!
//! ```ignore
//! use location::{current, watch};
//!
//! # async fn demo() -> Result<(), location::LocationError> {
//! // One fix (prompts for permission if needed).
//! let here = current().await?;
//! println!("{}, {} (±{} m)", here.latitude, here.longitude, here.accuracy_m);
//!
//! // Continuous updates — keep the guard alive while you want them.
//! let watch = watch(|pos| {
//!     println!("moved to {}, {}", pos.latitude, pos.longitude);
//! });
//! // ... later ...
//! drop(watch); // stops updates
//! # Ok(())
//! # }
//! ```
//!
//! # Architecture
//!
//! The platform-agnostic surface ([`Position`], [`LocationError`],
//! [`LocationWatch`], [`current`], [`watch`]) lives here. Exactly one
//! cfg-gated backend module is compiled per target and supplies the `imp`
//! module the public API delegates to:
//!
//! - **web (wasm32)** — `navigator.geolocation` (`getCurrentPosition` /
//!   `watchPosition` / `clearWatch`).
//! - **iOS / macOS / tvOS** — `CLLocationManager` with a
//!   `CLLocationManagerDelegate` (objc2).
//! - **Android** — the framework `LocationManager` via JNI
//!   (`getLastKnownLocation`; `requestLocationUpdates` needs a Java
//!   `LocationListener` shim — see [`watch`]).
//!
//! Every other target (the host running `cargo test`, desktop Windows/Linux)
//! gets a stub that reports [`LocationError::NotSupported`].
//!
//! # Permissions
//!
//! The location-permission **grant** goes through the `permissions` SDK —
//! [`current`] requests [`Permission::LocationWhenInUse`] before reading a
//! fix and returns [`LocationError::NotAuthorized`] if denied. Only the
//! position-**data** APIs are called here directly. The app must declare the
//! platform usage-description / manifest entries (see the README); the CLI
//! injects them from this crate's `capabilities = ["location"]`.

#![deny(missing_docs)]

pub mod recipes;

use permissions::{request, Permission};

// ---------------------------------------------------------------------------
// async bridge for the callback / delegate native APIs (Apple's delegate,
// the web `getCurrentPosition` success/error closures). A single-value
// channel from "called on a later run-loop turn" to our `async fn`.
// ---------------------------------------------------------------------------
mod oneshot;

// ---------------------------------------------------------------------------
// Public types.
// ---------------------------------------------------------------------------

/// A single device position fix.
///
/// `latitude` / `longitude` are WGS-84 degrees; `accuracy_m` is the radius of
/// 68 % confidence in metres (always present). The remaining fields are
/// `None` where the platform / hardware didn't supply them on this fix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    /// Latitude in WGS-84 degrees, north-positive.
    pub latitude: f64,
    /// Longitude in WGS-84 degrees, east-positive.
    pub longitude: f64,
    /// Horizontal accuracy: radius of 68 % confidence, in metres.
    pub accuracy_m: f64,
    /// Altitude in metres above the WGS-84 ellipsoid, if available.
    pub altitude: Option<f64>,
    /// Heading (course) in degrees clockwise from true north (`0..360`), if
    /// available — typically only while moving.
    pub heading: Option<f64>,
    /// Ground speed in metres per second, if available.
    pub speed: Option<f64>,
    /// Fix timestamp in milliseconds since the Unix epoch.
    pub timestamp_ms: f64,
}

/// Why a location request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocationError {
    /// The user denied (or hasn't granted) the location permission. Re-prompt
    /// or send the user to OS settings; the position isn't readable until
    /// granted.
    NotAuthorized,
    /// Location services produced no fix — services off, no signal, a timeout,
    /// or a transient hardware error. The string carries the platform detail.
    Unavailable(String),
    /// This target has no geolocation backend (desktop host, the test runner).
    NotSupported,
}

impl std::fmt::Display for LocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LocationError::NotAuthorized => write!(f, "location permission not granted"),
            LocationError::Unavailable(msg) => write!(f, "location unavailable: {msg}"),
            LocationError::NotSupported => {
                write!(f, "location not supported on this platform")
            }
        }
    }
}

impl std::error::Error for LocationError {}

// ---------------------------------------------------------------------------
// The watch callback bound.
//
// The native (Apple) delegate and the Android listener fire on a run-loop /
// JNI thread, so the callback must be `Send` there. The web backend runs it
// on the single wasm thread inside a JS closure holding non-`Send` JS values,
// so `Send` is both unnecessary and unsatisfiable. One cfg'd marker trait
// keeps the public `watch` signature identical on every target.
// ---------------------------------------------------------------------------

/// The bound a [`watch`] callback must satisfy. Implemented automatically for
/// any matching closure — pass a `|pos| { .. }` closure; you never write
/// `impl PositionCallback` yourself.
///
/// `Send` on native/Android (the callback runs on the location thread), not
/// on web (it runs on the main thread). The closure is `Fn` (callable
/// repeatedly): a self-re-dispatching native callback must not require
/// `FnMut`.
#[cfg(not(target_arch = "wasm32"))]
pub trait PositionCallback: Fn(Position) + Send + 'static {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Fn(Position) + Send + 'static> PositionCallback for T {}

/// See the non-wasm definition; on web the `Send` bound is dropped.
#[cfg(target_arch = "wasm32")]
pub trait PositionCallback: Fn(Position) + 'static {}
#[cfg(target_arch = "wasm32")]
impl<T: Fn(Position) + 'static> PositionCallback for T {}

/// The boxed form backends receive. Mirrors [`PositionCallback`]'s cfg'd
/// `Send`-ness.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type BoxedCallback = Box<dyn Fn(Position) + Send + 'static>;
#[cfg(target_arch = "wasm32")]
pub(crate) type BoxedCallback = Box<dyn Fn(Position) + 'static>;

// ---------------------------------------------------------------------------
// Backend selector. Exactly one `imp` compiles per target; each supplies
// `current_fix()`, `start_watch()`, and a `WatchHandle` whose `Drop` stops
// updates. The fallback stub reports `NotSupported`.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod imp;

#[cfg(all(
    not(target_arch = "wasm32"),
    any(target_os = "ios", target_os = "macos", target_os = "tvos")
))]
#[path = "apple.rs"]
mod imp;

#[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
#[path = "android.rs"]
mod imp;

// Fallback for every target with no geolocation backend (desktop
// Windows/Linux, the host running `cargo test`).
#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "ios", target_os = "macos", target_os = "tvos")),
    not(target_os = "android")
))]
mod imp {
    use super::{BoxedCallback, LocationError, Position};

    /// No backend on this target — every fix is `NotSupported`.
    pub(crate) async fn current_fix() -> Result<Position, LocationError> {
        Err(LocationError::NotSupported)
    }

    /// A no-op handle; there are no updates to stop on an unsupported target.
    pub(crate) struct WatchHandle;

    /// `watch` is best-effort: on an unsupported target it installs nothing
    /// and the callback simply never fires.
    pub(crate) fn start_watch(_callback: BoxedCallback) -> WatchHandle {
        WatchHandle
    }
}

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

/// Request a single device position fix.
///
/// Requests [`Permission::LocationWhenInUse`] through the `permissions` SDK
/// first (surfacing the OS prompt when undetermined); returns
/// [`LocationError::NotAuthorized`] if the user denies it. With permission in
/// hand it reads one fix from the platform's location stack.
///
/// Resolves [`LocationError::Unavailable`] when services are off / there's no
/// signal / the platform times out, and [`LocationError::NotSupported`] on a
/// target with no geolocation backend.
pub async fn current() -> Result<Position, LocationError> {
    // The grant flow is owned by the `permissions` SDK — we never call the OS
    // location-permission API ourselves, only the position-data API below.
    // `is_granted()` is the strict gate: `Unsupported` (a platform with no
    // grant model, e.g. the desktop host) is NOT granted here, and `current`
    // then resolves `NotSupported` from the stub backend — honest, not faked.
    if !request(Permission::LocationWhenInUse).await.is_granted() {
        return Err(LocationError::NotAuthorized);
    }
    imp::current_fix().await
}

/// Stream continuous position updates into `callback` until the returned
/// [`LocationWatch`] is dropped.
///
/// Unlike [`current`], this does not itself await a permission grant (it's a
/// synchronous installer so it can be called from non-async UI code). Call
/// [`current`] — or `permissions::request(Permission::LocationWhenInUse)` —
/// first to ensure the grant; without it the platform delivers no updates and
/// the callback never fires. The callback runs on the location thread on
/// native/Android and the main thread on web — keep it fast.
#[must_use = "dropping the LocationWatch immediately stops updates"]
pub fn watch(callback: impl PositionCallback) -> LocationWatch {
    let boxed: BoxedCallback = Box::new(callback);
    LocationWatch {
        _handle: imp::start_watch(boxed),
    }
}

/// An RAII guard for an active [`watch`]. Updates flow for as long as this
/// value is alive; **dropping it stops them** and releases the native
/// location manager.
///
/// Hold it in your app state for the duration you want updates. Do **not**
/// `mem::forget` it — that leaks the native manager and leaves the device's
/// location hardware running.
///
/// Not `Send` on native targets — the underlying platform manager is tied to
/// the thread (run loop) that created it. Keep it on that thread.
pub struct LocationWatch {
    // The concrete handle is backend-specific; its `Drop` stops updates.
    _handle: imp::WatchHandle,
}

#[cfg(test)]
mod tests {
    use super::*;

    // NB: `current()` is intentionally NOT unit-tested against the host. Its
    // first step awaits `permissions::request(LocationWhenInUse)`, which on a
    // macOS host runs the real `CLLocationManager` authorization path (not a
    // stub) — that's outside this crate's control and platform-dependent. The
    // permission-gate logic that IS ours (return `NotAuthorized` unless
    // granted; otherwise delegate to the backend) is straight-line and covered
    // by reading the code; the device behavior is the compile-checked native
    // path. We test the deterministic, host-owned surface below.

    /// `watch` installs without panicking on every target and hands back a
    /// guard; on the host the stub simply never fires the callback. The guard
    /// drops cleanly (no `mem::forget`, no leak).
    #[test]
    fn host_watch_installs_and_drops() {
        let guard = watch(|_pos| {
            // never fires on the host stub backend
        });
        drop(guard);
    }

    /// `Position` is the plain value type callers thread through their app —
    /// it's `Copy` + `PartialEq` so it slots into reactive state cheaply.
    #[test]
    fn position_is_a_plain_copy_value() {
        let p = Position {
            latitude: 37.7749,
            longitude: -122.4194,
            accuracy_m: 5.0,
            altitude: Some(16.0),
            heading: None,
            speed: Some(0.0),
            timestamp_ms: 1_700_000_000_000.0,
        };
        let q = p; // Copy
        assert_eq!(p, q);
        assert_eq!(q.latitude, 37.7749);
    }

    /// Errors render readable, distinct messages (the `Display`/`Error` impl
    /// callers log).
    #[test]
    fn error_display_is_readable() {
        assert_eq!(
            LocationError::NotAuthorized.to_string(),
            "location permission not granted"
        );
        assert!(LocationError::Unavailable("no fix".into())
            .to_string()
            .contains("no fix"));
        assert_eq!(
            LocationError::NotSupported.to_string(),
            "location not supported on this platform"
        );
    }
}
