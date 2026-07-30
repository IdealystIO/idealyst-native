//! `maps` — third-party `MapView` primitive for the framework.
//!
//! Demonstrates the third-party extension pattern: a shared types
//! crate (`maps-core`), this umbrella facade, and per-backend leaf
//! crates (`maps-web`, `maps-ios`, future `maps-android`) wired
//! together via target-specific Cargo dependencies. User code stays
//! target-agnostic; the umbrella selects the right leaf at compile
//! time.
//!
//! # Implementation
//!
//! The scene [`Registry`] is the runtime's unified primitive==external
//! contract, so the SDK registers a payload handler there:
//!
//! - **Web (wasm32)** — [`register`] installs a `WebBackend`-concrete
//!   handler that delegates to the `maps-web` leaf: the OpenStreetMap
//!   embed iframe built by `maps_web::build_map_iframe`, followed by
//!   author style via `attach_style` and the scope-tied
//!   `release_external` teardown.
//! - **iOS** — [`register`] installs an `IosBackend`-concrete handler
//!   that delegates to the `maps-ios` leaf: a native `MKMapView`
//!   (`maps_ios::build_map_view`) centered + zoomed from the props and
//!   registered with the backend's layout tree, then the same mount
//!   tail.
//! - **Everywhere else** — [`register`] installs the
//!   External-placeholder degradation path
//!   ([`ExternalOps::create_external`]): the host's "not supported" box
//!   (a bare `<div>` on SSR), with author style + `release_external`
//!   teardown still flowing.
//!
//! No handler installs DOM/platform event listeners or invokes author
//! callbacks (the props are plain `Copy` data), so the "external glue
//! must call `schedule_flush` after author callbacks" rule has no call
//! site in this SDK. A future leaf that DOES run author code from a raw
//! platform event must wrap it with the backend's `schedule_flush`.
//!
//! # Usage
//!
//! ```ignore
//! // In the app's bootstrap — the boot entry's `register` argument IS
//! // the registration seam (one line per third-party SDK):
//! backend_web::newcore::start_in("#app", maps::register, app);
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
//! An UNREGISTERED payload panics at realize (the scene contract), so a
//! missed `register` fails loud.
#![deny(missing_docs)]

// `Any` is only named by the placeholder arm (the payload type-erasure
// `create_external` takes).
#[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::{item, Element, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{
    attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
};

pub use maps_core::MapViewProps;

// ============================================================================
// Payload + builder — `MapView(props)` + `.with_style(…)` then element
// coercion. No ref plumbing: the props are plain data and the leaves
// expose no imperative ops, so no consumer binds a ref on a map.
// ============================================================================

/// Scene payload for the `MapView` item. Single-take style slot (the
/// vocabulary `PrimCell` discipline, inlined): the scene hands the
/// handler a shared `&Rc<Self>`, but the `StyleProp` must move at
/// mount.
struct MapsPrim {
    props: Rc<MapViewProps>,
    style: RefCell<Option<StyleProp>>,
}

/// Author-side builder returned by [`MapView`].
pub struct MapViewBound {
    props: Rc<MapViewProps>,
    style: Option<StyleProp>,
}

/// Construct a map view primitive.
///
/// PascalCase intentionally — matches the visual cadence of first-
/// party primitives inside a `ui!` block. Interpolate with
/// `{ MapView(MapViewProps { .. }) }`.
#[allow(non_snake_case)]
pub fn MapView(props: MapViewProps) -> MapViewBound {
    MapViewBound {
        props: Rc::new(props),
        style: None,
    }
}

impl MapViewBound {
    /// Attach the author style — lands on the outer node (the iframe on
    /// web, the `MKMapView` on iOS).
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }
}

impl IntoElement for MapViewBound {
    fn into_element(self) -> Element {
        item(
            MapsPrim {
                props: self.props,
                style: RefCell::new(self.style),
            },
            Vec::new(),
        )
    }
}

/// Element coercion for the constructor form.
impl From<MapViewBound> for Element {
    fn from(b: MapViewBound) -> Element {
        b.into_element()
    }
}

// ============================================================================
// Handlers + registration seam
// ============================================================================

/// Shared mount tail, run after node creation: (children are none for
/// maps) → author style → scope-tied `release_external`.
fn finish_mount<H>(backend: &Rc<RefCell<H>>, node: &H::Node, prim: &MapsPrim)
where
    H: ExternalOps + StyleServices,
{
    if let Some(style) = prim.style.borrow_mut().take() {
        attach_style(backend, node, style);
    }
    let backend = backend.clone();
    let node = node.clone();
    on_teardown(move || {
        backend.borrow_mut().release_external(&node);
    });
}

/// Placeholder handler for hosts with no map leaf — the External
/// degradation path (`create_external` renders each host's "not
/// supported" box; SSR renders a bare `<div>`).
#[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
fn mount_placeholder<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<MapsPrim>,
    _children: Vec<Element>,
) -> H::Node
where
    H: ExternalOps + StyleServices,
{
    let backend = cx.backend().clone();
    let payload: Rc<dyn Any> = prim.props.clone();
    let node = backend.borrow_mut().create_external(
        std::any::TypeId::of::<MapViewProps>(),
        std::any::type_name::<MapViewProps>(),
        &payload,
        &runtime_shared::accessibility::AccessibilityProps::default(),
    );
    finish_mount(&backend, &node, prim);
    node
}

/// Register the maps payload handler on a scene registry. Pass this as
/// the boot registration seam (the `register` argument of
/// `backend_web::newcore::start_in` / `backend_ssr::newcore::
/// render_path_with`).
#[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    registry.register::<MapsPrim, _>(mount_placeholder::<H>);
}

/// Register the maps payload handler on the web backend's scene
/// registry — the real OpenStreetMap iframe renderer (the `maps-web`
/// leaf's DOM).
#[cfg(target_arch = "wasm32")]
pub fn register(registry: &mut Registry<backend_web::WebBackend>) {
    registry.register::<MapsPrim, _>(mount_map_web);
}

/// Web mount handler: build the embed iframe (the leaf's builder), then
/// the standard mount tail (author style + teardown).
#[cfg(target_arch = "wasm32")]
fn mount_map_web(
    cx: &mut MountCx<'_, backend_web::WebBackend>,
    prim: &Rc<MapsPrim>,
    _children: Vec<Element>,
) -> web_sys::Node {
    let backend = cx.backend().clone();
    let node: web_sys::Node = maps_web::build_map_iframe(&prim.props).into();
    finish_mount(&backend, &node, prim);
    node
}

/// Register the maps payload handler on the iOS backend's scene
/// registry — the real `MKMapView` renderer (the `maps-ios` leaf).
#[cfg(target_os = "ios")]
pub fn register(registry: &mut Registry<backend_ios::IosBackend>) {
    registry.register::<MapsPrim, _>(mount_map_ios);
}

/// iOS mount handler: build the `MKMapView` (the leaf's builder, which
/// also registers the view with the backend's layout tree), then the
/// standard mount tail (author style + teardown).
#[cfg(target_os = "ios")]
fn mount_map_ios(
    cx: &mut MountCx<'_, backend_ios::IosBackend>,
    prim: &Rc<MapsPrim>,
    _children: Vec<Element>,
) -> backend_ios::IosNode {
    let backend = cx.backend().clone();
    let node = {
        let mut b = backend.borrow_mut();
        maps_ios::build_map_view(&prim.props, &mut b)
    };
    finish_mount(&backend, &node, prim);
    node
}
