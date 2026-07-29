//! New-core surface — the scene-registry leg (idea-lite migration).
//!
//! The old-core leg is an `Element::External` payload dispatched through
//! the per-backend `ExternalRegistry`; that registry does not exist on
//! the new core. The scene [`Registry`] is the new core's unified
//! primitive==external contract, so this leg registers a payload handler
//! there instead:
//!
//! - **Web (wasm32)** — [`register`] installs a `WebBackend`-concrete
//!   handler that delegates to the `maps-web` leaf's new-core leg: the
//!   OpenStreetMap embed iframe built by
//!   `maps_web::build_map_iframe` — the old handler's DOM,
//!   call-for-call, shared with the old-core leg so the two cores can't
//!   drift — followed by author style via [`attach_style`] and the
//!   scope-tied `release_external` teardown. The handler installs NO
//!   DOM event listeners and invokes NO author callbacks (the props are
//!   plain `Copy` data), so the "External web glue must call
//!   `schedule_flush` after author callbacks" residual (named in
//!   backend-web's `newcore.rs` module docs) has no call site here.
//! - **Everywhere else** — [`register`] installs the frozen
//!   External-placeholder degradation path
//!   ([`ExternalOps::create_external`]), mirroring the old walker's
//!   posture for a backend with no registered handler (author style +
//!   `release_external` teardown still flow, exactly like
//!   `walker/external.rs`). The `maps-ios` native `MKMapView` handler
//!   is old-core-only until the new-core native boots grow an external
//!   story — the codeblock precedent. When it is ported, the native
//!   external glue must wrap author callbacks with the backend's
//!   `schedule_flush` — the residual named in each backend's
//!   `newcore.rs` module docs.
//!
//! Same authored call shape as the old-core leg: `MapView(MapViewProps
//! { .. })` (+ `.with_style(…)`) then element coercion. The old core
//! returned the generic `Bound<ExternalHandle<MapViewProps>>`; the new
//! core has no `ExternalHandle`, and no consumer binds a ref on a map
//! (the props are plain data and the leaves expose no imperative ops),
//! so this leg keeps exactly the consumed surface: constructor + style.
//! App bootstrap passes [`register`] as the boot registration seam
//! (there is no inventory self-registration on the new core; the
//! registry is built explicitly at boot).

// `Any` is only named by the non-wasm placeholder arm (the payload
// type-erasure `create_external` takes).
#[cfg(not(target_arch = "wasm32"))]
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
// Payload + builder — the old `Bound<ExternalHandle<MapViewProps>>`
// call shape, narrowed to what call sites actually use
// (`.with_style(…)` then element coercion).
// ============================================================================

/// Scene payload for the `MapView` item. Single-take style slot (the
/// vocabulary `PrimCell` discipline, inlined): the scene hands the
/// handler a shared `&Rc<Self>`, but the `StyleProp` must move at
/// mount.
struct MapsPrim {
    props: Rc<MapViewProps>,
    style: RefCell<Option<StyleProp>>,
}

/// Author-side builder returned by [`MapView`] — mirrors the old-core
/// `Bound<ExternalHandle<MapViewProps>>` call shape.
pub struct MapViewBound {
    props: Rc<MapViewProps>,
    style: Option<StyleProp>,
}

/// Construct a map view primitive — the new-core stand-in for the
/// old-core `MapView(props)`, same call shape at every author site.
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
    /// web), exactly like `.with_style` on the old-core `Bound`.
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

/// Mirror of the old core's `From<Bound<H>> for Element`.
impl From<MapViewBound> for Element {
    fn from(b: MapViewBound) -> Element {
        b.into_element()
    }
}

// ============================================================================
// Handlers + registration seam
// ============================================================================

/// Shared mount tail — the old walker's `external.rs` sequence after
/// node creation: (children are none for maps) → author style →
/// scope-tied `release_external`. No ref plumbing — see the module
/// docs.
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

/// Placeholder handler for hosts with no real map renderer — the frozen
/// External degradation path (`create_external` with no registered
/// old-core handler renders each backend's "not supported" box; SSR
/// renders a bare `<div>`).
#[cfg(not(target_arch = "wasm32"))]
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
        &runtime_core::accessibility::AccessibilityProps::default(),
    );
    finish_mount(&backend, &node, prim);
    node
}

/// Register the maps payload handler on a scene registry. Pass this as
/// the boot registration seam (the `register` argument of
/// `backend_web::newcore::start_in` / `backend_ssr::newcore::
/// render_path_with`).
#[cfg(not(target_arch = "wasm32"))]
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    registry.register::<MapsPrim, _>(mount_placeholder::<H>);
}

/// Register the maps payload handler on the web backend's scene
/// registry — the real OpenStreetMap iframe renderer (the `maps-web`
/// leaf's DOM, shared with the old-core leg).
#[cfg(target_arch = "wasm32")]
pub fn register(registry: &mut Registry<backend_web::WebBackend>) {
    registry.register::<MapsPrim, _>(mount_map_web);
}

/// Web mount handler — the old maps-web handler call-for-call: build
/// the embed iframe (shared builder), then the standard external mount
/// tail (author style + teardown).
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
