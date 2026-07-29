//! Old-core suite: pins the `Element::External` lowering of the
//! authored surface (the byte-moved `oldcore` module) so the dual-core
//! restructure can't drift the default build.
#![cfg(not(feature = "new-core"))]

use std::any::TypeId;

use maps::{MapView, MapViewProps};
use runtime_core::Element;

/// `MapView(..)` lowers to `Element::External` keyed by
/// `MapViewProps`'s TypeId (backend handlers dispatch on it) and starts
/// childless, with the props riding the type-erased payload intact.
#[test]
fn map_view_builds_external_keyed_by_map_view_props() {
    let el: Element = MapView(MapViewProps {
        lat: 37.7749,
        lon: -122.4194,
        zoom: 12.0,
    })
    .into();
    match el {
        Element::External {
            type_id,
            type_name,
            payload,
            children,
            ..
        } => {
            assert_eq!(type_id, TypeId::of::<MapViewProps>());
            assert!(type_name.contains("MapViewProps"));
            assert!(children.is_empty(), "no children by default");
            let props = payload
                .downcast_ref::<MapViewProps>()
                .expect("payload is MapViewProps");
            assert_eq!(props.lat, 37.7749);
            assert_eq!(props.lon, -122.4194);
            assert_eq!(props.zoom, 12.0);
        }
        _ => panic!("MapView must lower to Element::External"),
    }
}
