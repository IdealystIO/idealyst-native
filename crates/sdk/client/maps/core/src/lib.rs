//! Shared types for the `maps` SDK. Pure data, zero platform deps.
//!
//! This is the base of the multi-crate `maps` split. It defines the
//! scene payload type ([`MapViewProps`]) and nothing else — no backend
//! code, no FFI, no node builder. It exists so the umbrella crate
//! (`maps`) and the per-backend leaves (`maps-web`, `maps-ios`, future
//! `maps-android`) can each depend on the shared props type without
//! forming a dependency cycle: the leaves build a node FROM
//! `MapViewProps`, and the umbrella needs each leaf's builder. Both
//! depend on this crate; this crate depends on neither.
//!
//! The author-facing `MapView` constructor, the cfg-routed `register`,
//! and the usage docs all live in the umbrella `maps` crate.
//!
//! The only non-data dependency here is `runtime-core`, pulled in for
//! the [`IdealystSchema`] derive so the catalog can document
//! `MapViewProps`. That derive is a no-op without the `catalog`
//! feature, so the "zero platform deps" property is preserved in a
//! normal build.
#![deny(missing_docs)]

use runtime_core::IdealystSchema;

/// Props for the `MapView` primitive. The umbrella's registered scene
/// handler consumes the fields directly to drive the platform's map SDK
/// (an OpenStreetMap embed iframe on web, `MKMapView` on iOS, etc.).
///
/// Plain `Copy` data — unlike the `webview`/`video`/`svg` props, there
/// are no reactive closures or callbacks here. A re-render with changed
/// coordinates rebuilds the native view rather than mutating it in
/// place; that's acceptable for the current POC leaves.
#[derive(Clone, Copy, Debug, IdealystSchema)]
pub struct MapViewProps {
    /// Latitude of the map center, in decimal degrees.
    #[schema(constraint = "-90.0 ..= 90.0")]
    pub lat: f64,
    /// Longitude of the map center, in decimal degrees.
    #[schema(constraint = "-180.0 ..= 180.0")]
    pub lon: f64,
    /// Zoom level; semantics match the OpenStreetMap convention
    /// (0 = world, 18 ≈ street). Leaf crates clamp to their native
    /// SDK's range.
    #[schema(constraint = "0.0 ..= 18.0 (OSM tile zoom)")]
    pub zoom: f32,
}
