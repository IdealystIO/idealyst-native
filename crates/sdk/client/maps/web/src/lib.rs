//! Web leaf for the `maps` SDK: the OpenStreetMap embed-iframe node
//! builder the umbrella's `WebBackend`-concrete scene handler mounts.
//!
//! This is one per-backend leaf of the multi-crate `maps` split: it
//! depends on `maps-core` for the shared [`MapViewProps`](maps_core::MapViewProps)
//! type and nothing else. The author never names this crate — the
//! umbrella `maps` crate pulls it in under
//! `[target.'cfg(target_arch = "wasm32")'.dependencies]` and calls
//! [`build_map_element`] from its registered handler, so app code only
//! ever passes `maps::register` at the boot seam.
//!
//! Using an `<iframe>` here is a POC choice — it shows a real map
//! with zero FFI ceremony. A production version would bind to
//! Leaflet / MapLibre through web-glue bindings so the map is interactive at
//! the Rust API level (markers, animated camera moves, etc.).

use maps_core::MapViewProps;

/// The attributes every map iframe carries, `src` first. One list shared by
/// both builders so the glue and the web-sys element cannot drift.
fn iframe_attributes(props: &MapViewProps) -> [(&'static str, String); 5] {
    [
        ("src", embed_src(props)),
        ("loading", "lazy".to_string()),
        ("referrerpolicy", "no-referrer".to_string()),
        // Only `border: 0` inline: an inline `width`/`height` beats the
        // author's style classes, so `MapView(..).with_style(..)` could
        // never size the map on web (it used to pin `width: 100%;
        // height: 100%; min-height: 300px` here). Size is the author's, as
        // with the webview SDK's iframe and the native map views.
        ("style", "border: 0".to_string()),
        ("data-external-kind", "maps_core::MapViewProps".to_string()),
    ]
}

/// The OpenStreetMap embed URL centered on the requested lat/lon at the
/// requested zoom. The bounding-box width shrinks as zoom increases (rough
/// heuristic — production code would drive Leaflet's `setView` API
/// directly instead of recomputing a bbox per render).
fn embed_src(props: &MapViewProps) -> String {
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

    format!(
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
    )
}

/// Build an `<iframe>` embedding OpenStreetMap centered on the requested
/// lat/lon at the requested zoom, as a web-glue handle — the web
/// backend's own node type (`Host::Node` is `web_glue::dom::Node`), which
/// the umbrella's scene handler mounts directly.
pub fn build_map_element(props: &std::rc::Rc<MapViewProps>) -> web_glue::dom::Element {
    let document = web_glue::dom::window()
        .expect("no window")
        .document()
        .expect("no document");
    let iframe = document
        .create_element("iframe")
        .expect("create_element(iframe) failed");
    for (name, value) in iframe_attributes(props) {
        let _ = iframe.set_attribute(name, &value);
    }
    iframe
}

/// [`build_map_element`] as a `web_sys::Element`.
///
/// Kept so this published crate's API does not change; the umbrella no
/// longer calls it. It is the only reason this crate still depends on
/// web-sys (the framework's web layer runs on web-glue since
/// own-web-bindings phase 3).
#[deprecated(note = "use `build_map_element`, which returns the web backend's own node type")]
pub fn build_map_iframe(props: &std::rc::Rc<MapViewProps>) -> web_sys::Element {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");
    let iframe = document
        .create_element("iframe")
        .expect("create_element(iframe) failed");
    for (name, value) in iframe_attributes(props) {
        let _ = iframe.set_attribute(name, &value);
    }
    iframe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_src_centers_the_bbox_and_marks_the_point() {
        let props = MapViewProps { lat: 10.0, lon: 20.0, zoom: 1.0 };
        let src = embed_src(&props);
        // zoom 1: lat span 45, lon span 90.
        assert!(src.starts_with("https://www.openstreetmap.org/export/embed.html?bbox=-70,-35,110,55&"), "{src}");
        assert!(src.ends_with("&layer=mapnik&marker=10,20"), "{src}");
    }

    #[test]
    fn both_builders_share_one_attribute_list() {
        let attrs = iframe_attributes(&MapViewProps { lat: 0.0, lon: 0.0, zoom: 3.0 });
        let names: Vec<_> = attrs.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["src", "loading", "referrerpolicy", "style", "data-external-kind"]);
        assert_eq!(attrs[3].1, "border: 0", "no inline size: it would beat the author's style");
        assert_eq!(attrs[4].1, "maps_core::MapViewProps");
    }
}
