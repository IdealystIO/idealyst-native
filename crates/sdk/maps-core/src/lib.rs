//! Shared types for the `maps` SDK. Pure data, zero platform deps.
//!
//! Lives in its own crate so per-backend leaves (`maps-web`, future
//! `maps-ios`/`maps-android`) and the umbrella crate (`maps`) can both
//! depend on this without forming a cycle.

/// Props for the `map_view` external primitive. Backends register a
/// handler keyed by `TypeId::of::<MapViewProps>()` and consume the
/// fields directly to drive their native map SDK (Leaflet via DOM
/// iframe on web, MKMapView on iOS, etc.).
#[derive(Clone, Debug)]
pub struct MapViewProps {
    pub lat: f64,
    pub lon: f64,
    /// Zoom level; semantics match the OpenStreetMap convention
    /// (0 = world, 18 ≈ street). Leaf crates clamp to their native
    /// SDK's range.
    pub zoom: f32,
}
