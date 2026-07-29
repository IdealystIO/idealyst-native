//! `maps` — third-party `MapView` primitive for the framework.
//!
//! Demonstrates the third-party extension pattern: a shared types
//! crate (`maps-core`), this umbrella facade, and per-backend leaf
//! crates (`maps-web`, `maps-ios`, future `maps-android`) wired
//! together via target-specific Cargo dependencies. User code stays
//! target-agnostic; the umbrella selects the right leaf at compile
//! time.
//!
//! # One authored surface, two cores
//!
//! The default build is the old-core implementation (`Element::External`
//! payload + per-backend `ExternalRegistry`), byte-moved into
//! [`oldcore`]; the `new-core` feature swaps in [`newcore`], which
//! re-expresses the SAME public call shape (`MapView(MapViewProps
//! { .. })` + a bootstrap `register`) over the scene registry — the new
//! core's unified primitive==external contract. Mutually exclusive
//! (same names), mirroring the svg/codeblock/table SDK precedents. The
//! new-core web leg reproduces the old maps-web handler's iframe DOM
//! call-for-call (the DOM builder is shared with the old leg via
//! `maps_web::build_map_iframe`); on non-web new-core hosts the SDK
//! registers the frozen External-placeholder degradation path
//! (`maps-ios`'s native `MKMapView` handler is old-core-only until the
//! new-core native boots grow an external story — see `newcore.rs`).
//!
//! # Usage (old core)
//!
//! ```ignore
//! // In the app's bootstrap (one line per third-party SDK):
//! let mut backend = WebBackend::new("#app");
//! maps::register(&mut backend);
//!
//! // Inside a `ui!` block. Third-party primitives don't get block
//! // syntax (the macro only recognizes the first-party set), so the
//! // constructor is interpolated as an expression — but the
//! // PascalCase name reads identically to a first-party `overlay { }`
//! // or `view { }`.
//! ui! {
//!     view {
//!         { MapView(MapViewProps { lat: 37.7749, lon: -122.4194, zoom: 12.0 }) }
//!     }
//! }
//! ```
//!
//! On platforms with no matching leaf crate, `register` installs the
//! placeholder path and the framework renders a "not supported" box
//! when the primitive mounts.
//!
//! On the new core the same author code compiles unchanged; only the
//! bootstrap seam moves — pass [`register`] to the boot entry
//! (`backend_web::newcore::start_in("#app", maps::register, app)`).
#![deny(missing_docs)]

// One authored surface, two cores: the default build is the old-core
// implementation, byte-moved into `oldcore`; the `new-core` feature
// swaps in `newcore`, which re-expresses the SAME public names over the
// scene registry. Mutually exclusive (same names).
#[cfg(not(feature = "new-core"))]
mod oldcore;
#[cfg(not(feature = "new-core"))]
pub use oldcore::*;

#[cfg(feature = "new-core")]
mod newcore;
#[cfg(feature = "new-core")]
pub use newcore::*;
