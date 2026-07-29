//! New-core leg of the "private layer" — the scene-registry
//! re-expression of `private_layer.rs`'s `Element::External` surface
//! (idea-lite migration). Selected by the `new-core` feature via the
//! `#[path]` swap in lib.rs, so the module path
//! (`screen_recorder::private_layer`) and the public names
//! ([`PrivateLayerProps`], [`PrivateLayer`]) are identical on both
//! cores.
//!
//! # New-core lowering: passthrough container on every target
//!
//! The old core's per-platform capture-EXCLUDED windows (the separate
//! `UIWindow` on iOS / `WindowManager` window on Android / `NSPanel` on
//! macOS) are built through each backend's old-core `ExternalRegistry`
//! handler and remain **old-core-only** — that is the documented seam:
//! the new-core native boots don't have an external-window story yet,
//! and porting those handlers means re-plumbing
//! `create_private_layer_window` + the detached-window-root bookkeeping
//! through the scene contract (plus routing author callbacks through
//! each platform's post-dispatch flush seam).
//!
//! Until then, the new-core `PrivateLayer` renders its children in a
//! plain passthrough container on EVERY target, mounted by the handler
//! [`register_scene`] installs: `create_external` (the placeholder
//! path) → children realized INTO the returned node → scope-tied
//! `release_external`. This matches the old core's web posture exactly
//! (the old web handler was the documented inline no-op — children
//! render inline and ARE captured) and the placeholder posture
//! everywhere a real handler wasn't registered.
//!
//! The capture capability itself ([`crate::ScreenRecorder`]) is
//! core-agnostic and unchanged.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::{item, Element, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{on_teardown, StyleServices};

/// Marker payload for the private layer. No fields yet — the layer's
/// behavior is entirely in the registered handler. Kept as a named
/// struct so its [`std::any::TypeId`] is the dispatch key (the same
/// role it plays on the old core).
pub struct PrivateLayerProps {
    _private: (),
}

impl Default for PrivateLayerProps {
    fn default() -> Self {
        Self { _private: () }
    }
}

/// Wrap `children` in the private-layer surface.
///
/// Same call shape as the old core (`PrivateLayer(vec![…])` inside a
/// `ui!`/`jsx!` interpolation). On the new core this lowers to a scene
/// item whose handler mounts a passthrough container — the children
/// render inline and ARE captured (the old-core web posture); the
/// capture-excluded native windows are old-core-only for now (see the
/// module docs). Divergence from the old core: this returns an opaque
/// element builder, not a `Bound` — the old `.with_style(…)` chain on
/// the returned value had no documented consumer and is dropped.
#[allow(non_snake_case)]
pub fn PrivateLayer(children: Vec<Element>) -> impl IntoElement {
    PrivateLayerBound { children }
}

/// Deferred build for [`PrivateLayer`] — carries the children into the
/// scene item's children slot (the handler parents them, mirroring the
/// old walker's `insert_children` contract).
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
/// `create_external` receives the same `Rc<PrivateLayerProps>` payload
/// the old walker passed.
struct PrivateLayerPrim {
    props: Rc<PrivateLayerProps>,
}

/// Mount the passthrough container: `create_external` keyed by
/// [`PrivateLayerProps`], children realized INTO the returned node,
/// scope-tied `release_external` — the old walker's `external.rs` order
/// (no author style / ref slots on this surface).
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
        &runtime_core::accessibility::AccessibilityProps::default(),
    );
    cx.realize_children_into(&mut node, children);
    let backend_for_teardown: Rc<RefCell<H>> = backend.clone();
    let node_for_teardown = node.clone();
    on_teardown(move || {
        backend_for_teardown
            .borrow_mut()
            .release_external(&node_for_teardown);
    });
    node
}

/// Register the private-layer handler on a scene registry — the
/// new-core boot seam (pass alongside the app's other SDK registrations
/// to `backend_web::newcore::start_in` / the platform boot entry).
/// Named `register_scene` because the crate's existing [`crate::register`]
/// is the old-core per-backend bootstrap (kept as a no-op on this leg
/// so same-source bootstrap code compiles unchanged).
pub fn register_scene<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    registry.register::<PrivateLayerPrim, _>(mount_passthrough::<H>);
}
