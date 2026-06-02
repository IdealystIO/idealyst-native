//! `maps` — third-party `MapView` primitive for the framework.
//!
//! Demonstrates the third-party extension pattern: a shared types
//! crate (`maps-core`), this umbrella facade, and per-backend leaf
//! crates (`maps-web`, future `maps-ios`/`maps-android`) wired together
//! via target-specific Cargo dependencies. User code stays target-
//! agnostic; the umbrella selects the right leaf at compile time.
//!
//! # Usage
//!
//! ```ignore
//! // In the app's bootstrap (one line per third-party SDK):
//! let mut backend = WebBackend::new("#app");
//! maps::register(&mut backend);
//!
//! // Inside a `ui!` block. Third-party primitives don't get block
//! // syntax (the macro only recognizes the first-party set), so the
//! // constructor is interpolated as an expression — but the
//! // PascalCase name reads identically to a first-party `Overlay { }`
//! // or `View { }`.
//! ui! {
//!     View {
//!         { MapView(MapViewProps { lat: 37.7749, lon: -122.4194, zoom: 12.0 }) }
//!     }
//! }
//! ```
//!
//! On platforms with no matching leaf crate, `register` is a no-op and
//! the framework renders a "not supported" placeholder when the
//! primitive mounts.
#![deny(missing_docs)]

use runtime_core::{external, Bound, ExternalHandle};

pub use maps_core::MapViewProps;

/// Construct a map view primitive. Returns a typed `Bound` so
/// `.bind(...)` is type-checked against `Ref<ExternalHandle<MapViewProps>>`.
///
/// PascalCase intentionally — matches the visual cadence of first-
/// party primitives (`View`, `Overlay`, `Button`) inside a `ui!`
/// block. Interpolate with `{ MapView(MapViewProps { .. }) }`.
#[allow(non_snake_case)]
pub fn MapView(props: MapViewProps) -> Bound<ExternalHandle<MapViewProps>> {
    external(props)
}

// =============================================================================
// Platform-routed `register` re-export.
//
// Exactly one of the cfg-gated re-exports is active per build, selected
// by `target_arch` / `target_os`. Each leaf crate's `register` function
// takes the platform-specific backend type by mutable reference and
// calls `backend.register_external::<MapViewProps>(...)`.
//
// The fallback `pub fn register<B>(_: &mut B) {}` covers targets we
// haven't shipped a leaf for; user code compiles uniformly across all
// targets and the framework's placeholder shows up at runtime.
// =============================================================================

#[cfg(target_arch = "wasm32")]
pub use maps_web::register;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub use maps_ios::register;

/// No-op `register` for targets without a `maps` leaf crate. User code
/// calls this unconditionally; the framework's "External MapViewProps
/// not supported" placeholder shows up at runtime to make the missing
/// binding obvious.
#[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
pub fn register<B>(_backend: &mut B) {
    // No leaf available for this target — the framework will render
    // its "External MapViewProps not supported" placeholder at mount.
}
