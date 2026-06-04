//! Web leaf for the `maps` SDK. Registers a `MapViewProps` handler
//! against `WebBackend` that renders an OpenStreetMap embed iframe.
//!
//! This is one per-backend leaf of the multi-crate `maps` split: it
//! depends on `maps-core` for the shared [`MapViewProps`](maps_core::MapViewProps)
//! type and on `backend-web` for the concrete backend it registers
//! against. The author never names this crate — the umbrella `maps`
//! crate re-exports this leaf's [`register`] under
//! `[target.'cfg(target_arch = "wasm32")'.dependencies]`, so app code
//! calls `maps::register(&mut backend)` and Cargo routes it here on
//! web.
//!
//! Using an `<iframe>` here is a POC choice — it shows a real map
//! with zero FFI ceremony. A production version would bind to
//! Leaflet / MapLibre via wasm-bindgen so the map is interactive at
//! the Rust API level (markers, animated camera moves, etc.).

use backend_web::WebBackend;
use maps_core::MapViewProps;

/// Install the MapView handler. Called once at app bootstrap:
///
/// ```ignore
/// let mut backend = WebBackend::new("#app");
/// maps::register(&mut backend);   // routes to this function on web
/// ```
pub fn register(backend: &mut WebBackend) {
    backend.register_external::<MapViewProps, _>(|props, _backend| {
        build_map_iframe(props)
    });
}

// Self-register at backend construction. See [[project_inventory_self_registration]].
inventory::submit! {
    backend_web::WebExternalRegistrar(register)
}

/// Build an `<iframe>` embedding OpenStreetMap centered on the
/// requested lat/lon at the requested zoom. The bounding-box width
/// shrinks as zoom increases (rough heuristic — production code would
/// drive Leaflet's `setView` API directly instead of recomputing a
/// bbox per render).
fn build_map_iframe(props: &std::rc::Rc<MapViewProps>) -> web_sys::Element {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");

    let iframe = document
        .create_element("iframe")
        .expect("create_element(iframe) failed");

    // Approximate degree-span per axis at this zoom. OSM tiles use
    // a power-of-two scheme; 360° at zoom 0, halving per zoom level.
    // Multiply by 0.5 so the bbox shows ~one "tile equivalent" worth
    // around the center.
    let span_lat = 180.0 / 2f64.powf(props.zoom as f64) * 0.5;
    let span_lon = 360.0 / 2f64.powf(props.zoom as f64) * 0.5;
    let left = props.lon - span_lon;
    let right = props.lon + span_lon;
    let top = props.lat + span_lat;
    let bottom = props.lat - span_lat;

    let src = format!(
        "https://www.openstreetmap.org/export/embed.html\
         ?bbox={left},{bottom},{right},{top}\
         &layer=mapnik\
         &marker={lat},{lon}",
        left = left,
        bottom = bottom,
        right = right,
        top = top,
        lat = props.lat,
        lon = props.lon,
    );

    let _ = iframe.set_attribute("src", &src);
    let _ = iframe.set_attribute("loading", "lazy");
    let _ = iframe.set_attribute("referrerpolicy", "no-referrer");
    let _ = iframe.set_attribute(
        "style",
        "border: 0; width: 100%; height: 100%; min-height: 300px;",
    );
    let _ = iframe.set_attribute(
        "data-external-kind",
        "maps_core::MapViewProps",
    );

    iframe
}
