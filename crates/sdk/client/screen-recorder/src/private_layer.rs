//! The "private layer" — an overlay subtree that screen recordings do
//! NOT capture.
//!
//! # How exclusion works
//!
//! On every native desktop/mobile host the layer's children are parented
//! into a SEPARATE platform window that the capture path deliberately
//! omits:
//!
//! - **iOS** — a ReplayKit-excluded `UIWindow`
//!   (`IosBackend::create_private_layer_window`). ReplayKit records the
//!   app's key window only; the overlay lives on a non-key window at a
//!   high `windowLevel`, so the user sees it and the recording doesn't.
//! - **Android** — a `WindowManager` window outside the Activity's decor
//!   view (`AndroidBackend::create_private_layer_window`). MediaProjection
//!   capture here uses PixelCopy against the decor view, so the overlay
//!   is absent from the frames.
//! - **macOS** — a borderless child `NSWindow`
//!   (`MacosBackend::create_private_layer_window`) whose `windowNumber`
//!   the backend records; [`crate::ScreenRecorder::start`] reads those
//!   ids via `backend_macos::private_layer_window_ids` and passes the
//!   matching `SCWindow`s to the `SCContentFilter` exclusion list.
//!
//! Each of those windows has a passthrough `hitTest:` so clicks that
//! miss a real control fall through to the app beneath, and each content
//! view is registered as a detached window root so the backend's
//! `insert` / `clear_children` skip reparenting it into the recorded
//! tree.
//!
//! On **web** and on hosts with no capture-exclusion mechanism the
//! handler mounts a plain passthrough container instead: the children
//! render inline and ARE captured. (Web's eventual answer is the Element
//! Capture API's `RestrictionTarget.fromElement(...)` +
//! `track.restrictTo(target)`, Chromium-only — the node this handler
//! returns is the natural anchor once that is wired.)
//!
//! The capture capability itself ([`crate::ScreenRecorder`]) is
//! independent of this module.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::{item, Element, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{on_teardown, StyleServices};

/// Marker payload for the private layer. No fields — the layer's
/// behavior is entirely in the registered handler. Kept as a named
/// struct so its [`std::any::TypeId`] is the dispatch key.
pub struct PrivateLayerProps {
    _private: (),
}

impl Default for PrivateLayerProps {
    fn default() -> Self {
        Self { _private: () }
    }
}

/// Wrap `children` in the private-layer surface —
/// `PrivateLayer(vec![…])` inside a `ui!`/`jsx!` interpolation.
#[allow(non_snake_case)]
pub fn PrivateLayer(children: Vec<Element>) -> impl IntoElement {
    PrivateLayerBound { children }
}

/// Deferred build for [`PrivateLayer`] — carries the children into the
/// scene item's children slot (the handler parents them).
struct PrivateLayerBound {
    children: Vec<Element>,
}

impl IntoElement for PrivateLayerBound {
    fn into_element(self) -> Element {
        item(
            PrivateLayerPrim {
                props: Rc::new(PrivateLayerProps::default()),
            },
            self.children,
        )
    }
}

/// Scene payload — the registry dispatch key. Wraps the marker props so
/// the placeholder path hands `create_external` an
/// `Rc<PrivateLayerProps>`.
struct PrivateLayerPrim {
    props: Rc<PrivateLayerProps>,
}

/// Scope-tied `release_external`: on the native legs this is what tears
/// the overlay window down (`release_private_layer_window` runs inside
/// each backend's `release_external`), so it must fire for the concrete
/// handlers too, not only the placeholder.
fn release_on_teardown<H>(backend: &Rc<RefCell<H>>, node: &H::Node)
where
    H: ExternalOps,
{
    let backend = backend.clone();
    let node = node.clone();
    on_teardown(move || {
        backend.borrow_mut().release_external(&node);
    });
}

/// Passthrough-container handler for hosts with no capture-exclusion
/// mechanism: `create_external` keyed by [`PrivateLayerProps`], children
/// realized INTO the returned node, scope-tied `release_external` (no
/// author style / ref slots on this surface).
fn mount_passthrough<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<PrivateLayerPrim>,
    children: Vec<Element>,
) -> H::Node
where
    H: ExternalOps + StyleServices,
{
    let backend = cx.backend().clone();
    let payload: Rc<dyn Any> = prim.props.clone();
    let mut node = backend.borrow_mut().create_external(
        std::any::TypeId::of::<PrivateLayerProps>(),
        std::any::type_name::<PrivateLayerProps>(),
        &payload,
        &runtime_shared::accessibility::AccessibilityProps::default(),
    );
    cx.realize_children_into(&mut node, children);
    release_on_teardown(&backend, &node);
    node
}

/// iOS handler: the ReplayKit-excluded `UIWindow`'s content view, with
/// the layer's children realized into it.
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
fn mount_ios_window(
    cx: &mut MountCx<'_, backend_ios::IosBackend>,
    _prim: &Rc<PrivateLayerPrim>,
    children: Vec<Element>,
) -> backend_ios::IosNode {
    let backend = cx.backend().clone();
    let mut node = backend.borrow_mut().create_private_layer_window();
    cx.realize_children_into(&mut node, children);
    release_on_teardown(&backend, &node);
    node
}

/// macOS handler: the borderless child `NSWindow`'s content view, with
/// the layer's children realized into it.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
fn mount_macos_window(
    cx: &mut MountCx<'_, backend_macos::MacosBackend>,
    _prim: &Rc<PrivateLayerPrim>,
    children: Vec<Element>,
) -> backend_macos::MacosNode {
    let backend = cx.backend().clone();
    let mut node = backend.borrow_mut().create_private_layer_window();
    cx.realize_children_into(&mut node, children);
    release_on_teardown(&backend, &node);
    node
}

/// Android handler: the `WindowManager` window's content view, with the
/// layer's children realized into it.
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
fn mount_android_window(
    cx: &mut MountCx<'_, backend_android::AndroidBackend>,
    _prim: &Rc<PrivateLayerPrim>,
    children: Vec<Element>,
) -> jni::objects::GlobalRef {
    let backend = cx.backend().clone();
    let mut node = backend.borrow_mut().create_private_layer_window();
    cx.realize_children_into(&mut node, children);
    release_on_teardown(&backend, &node);
    node
}

/// Register the private-layer handler on a scene registry — pass this
/// alongside the app's other SDK registrations to the boot entry
/// (`backend_web::newcore::start_in`, `host_appkit::newcore::run_with`,
/// the mobile `run_in_view`, …).
///
/// # One `register`, resolved at registration time
///
/// The capture-excluded window is backend-CONCRETE (it is a real
/// platform window, not anything the caps traits can express), but a
/// native build must also serve `Registry<HostMock>` for the test
/// harness. So this fn stays generic and type-dispatches ONCE at
/// registration: it downcasts `&mut Registry<H>` to the platform's
/// concrete registry (`H: 'static` makes the registry `Any`) and
/// installs the real overlay-window handler on hit; every other `H` —
/// and every host without an exclusion mechanism — gets the passthrough
/// container. Mount-path cost: zero.
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    #[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_ios::IosBackend>>() {
            reg.register::<PrivateLayerPrim, _>(mount_ios_window);
            return;
        }
    }
    #[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_macos::MacosBackend>>() {
            reg.register::<PrivateLayerPrim, _>(mount_macos_window);
            return;
        }
    }
    #[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_android::AndroidBackend>>() {
            reg.register::<PrivateLayerPrim, _>(mount_android_window);
            return;
        }
    }
    registry.register::<PrivateLayerPrim, _>(mount_passthrough::<H>);
}
