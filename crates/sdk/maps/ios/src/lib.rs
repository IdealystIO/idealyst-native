//! iOS leaf for the `maps` SDK. Registers a `MapViewProps` handler
//! against `IosBackend` that mounts a native `MKMapView` centered on
//! the requested coordinate.
//!
//! This is one per-backend leaf of the multi-crate `maps` split: it
//! depends on `maps-core` for the shared [`MapViewProps`](maps_core::MapViewProps)
//! type and on `backend-ios` for the concrete backend it registers
//! against. The author never names this crate — the umbrella `maps`
//! crate re-exports this leaf's [`register`] under
//! `[target.'cfg(target_os = "ios")'.dependencies]`, so app code calls
//! `maps::register(&mut backend)` and Cargo routes it here on iOS.
//!
//! Reaches MKMapView at the Obj-C runtime layer via `AnyClass::get` +
//! raw `msg_send` rather than going through `objc2-map-kit` — same
//! rationale as webview-ios (see crates/sdk/webview/Cargo.toml): the
//! sub-crate's MainThreadMarker plumbing pins it to a different objc2
//! major and the backend can't co-host both. Calling MKMapView
//! selectors directly works fine; the host project still needs to
//! link `MapKit.framework` so the class is registered at startup.

#![cfg(target_os = "ios")]

use backend_ios::{IosBackend, IosNode};
use maps_core::MapViewProps;
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::{CGRect, NSObject};
use objc2_ui_kit::UIView;
use std::rc::Rc;

// CLLocationCoordinate2D from CoreLocation: `{ double lat; double lon; }`.
// Declared inline (rather than depending on objc2-core-location) because
// the layout is stable across Apple SDK versions and the encoding string
// must match MapKit's exact typename so `setCenterCoordinate:` /
// `cameraLookingAtCenterCoordinate:...` accept the value.
#[repr(C)]
#[derive(Clone, Copy)]
struct CLLocationCoordinate2D {
    latitude: f64,
    longitude: f64,
}

unsafe impl Encode for CLLocationCoordinate2D {
    const ENCODING: Encoding =
        Encoding::Struct("CLLocationCoordinate2D", &[f64::ENCODING, f64::ENCODING]);
}

unsafe impl RefEncode for CLLocationCoordinate2D {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

/// Install the MapView handler on the iOS backend. Called once at app
/// bootstrap:
///
/// ```ignore
/// let mut backend = IosBackend::new(...);
/// maps::register(&mut backend);   // routes to this function on iOS
/// ```
pub fn register(backend: &mut IosBackend) {
    backend.register_external::<MapViewProps, _>(|props, b| build_map_view(props, b));
}

/// Construct an `MKMapView` at zero rect (Taffy resizes it from the
/// layout pass), set its `centerCoordinate` + `camera.altitude` from
/// the props, register it with the backend's layout tree, and wrap it
/// as an `IosNode::View`.
///
/// Zoom is converted to camera altitude using the standard MapKit
/// formula: altitude in meters ≈ 591657550.5 / 2^zoom. The constant
/// is the equatorial circumference of Earth's tile-pyramid at zoom 0
/// (the value MKMapView itself uses internally).
fn build_map_view(props: &Rc<MapViewProps>, b: &mut IosBackend) -> IosNode {
    let mk_class: &AnyClass = AnyClass::get("MKMapView")
        .expect("MKMapView class not found — is MapKit linked into the app?");

    let zero_rect: CGRect = CGRect {
        origin: objc2_foundation::CGPoint { x: 0.0, y: 0.0 },
        size: objc2_foundation::CGSize { width: 0.0, height: 0.0 },
    };

    let map_any: Retained<NSObject> = unsafe {
        let allocated: *mut AnyObject = msg_send![mk_class, alloc];
        let inited: *mut AnyObject = msg_send![allocated, initWithFrame: zero_rect];
        Retained::from_raw(inited.cast::<NSObject>())
            .expect("MKMapView init returned nil")
    };
    let map_uiview: Retained<UIView> = unsafe { Retained::cast(map_any) };

    b.register_external_view(&map_uiview);

    let center = CLLocationCoordinate2D {
        latitude: props.lat,
        longitude: props.lon,
    };

    // `setCenterCoordinate:` sets the focus point but doesn't zoom;
    // for zoom we build an MKMapCamera with the derived altitude and
    // `setCamera:` so MapKit interpolates appropriately.
    let _: () = unsafe { msg_send![&*map_uiview, setCenterCoordinate: center] };

    let altitude_meters = 591_657_550.5_f64 / 2f64.powf(props.zoom as f64);
    let camera_class: &AnyClass = AnyClass::get("MKMapCamera")
        .expect("MKMapCamera class not found");
    let camera: Retained<NSObject> = unsafe {
        let cam: *mut AnyObject = msg_send![
            camera_class,
            cameraLookingAtCenterCoordinate: center,
            fromEyeCoordinate: center,
            eyeAltitude: altitude_meters,
        ];
        // `cameraLookingAtCenterCoordinate:...` is a class method that
        // returns an autoreleased instance — `retain` to take ownership
        // so it survives past the autorelease pool drain.
        let retained: *mut AnyObject = msg_send![cam, retain];
        Retained::from_raw(retained.cast::<NSObject>())
            .expect("MKMapCamera class method returned nil")
    };
    let _: () = unsafe { msg_send![&*map_uiview, setCamera: &*camera] };

    IosNode::View(map_uiview)
}
