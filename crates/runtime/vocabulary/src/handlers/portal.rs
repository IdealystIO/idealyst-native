//! Render-elsewhere handler: `portal` — port of `walker/portal.rs::build`
//! (which also serves the `overlay` / `anchored_overlay` compositions,
//! since those lower to a portal item at build time — see
//! [`crate::builders::overlay`]).
//!
//! Sequence (walker order preserved): `create_portal` → children →
//! attach_style → ref-fill → ScreenNav visibility effect →
//! `release_portal` teardown guard.

use runtime_scene::{Element, MountCx, Registry};
use runtime_world::{effect, inject};

use crate::caps::PortalOps;
use crate::prims::{PortalPrim, PrimCell, ScreenNav};
use crate::style_attach::{attach_style, on_teardown, StyleServices};

/// Mount a `portal`.
pub fn mount_portal<H>(cx: &mut MountCx<'_, H>, prim: PortalPrim, children: Vec<Element>) -> H::Node
where
    H: PortalOps + StyleServices,
{
    let backend = cx.backend().clone();
    let dismiss_for_backend = prim.on_dismiss.clone();
    let mut node = backend.borrow_mut().create_portal(
        prim.target,
        dismiss_for_backend,
        prim.trap_focus,
        &prim.a11y,
    );

    cx.realize_children_into(&mut node, children);

    if let Some(style) = prim.style {
        attach_style(&backend, &node, style);
    }

    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_portal_handle(&node);
        fill(handle);
    }

    // Hide this portal while its owning screen isn't the active route,
    // and show it again on return. A portal escapes its screen's view
    // tree to mount on the window, so it isn't detached when the
    // navigator swaps screens; with a persistent mount policy the
    // screen's scope (and thus this portal) also stays alive. Without
    // this, an overlay (modal / popover / click-away catcher) opened on
    // one screen keeps floating over the next. `ScreenNav` is provided
    // by the nearest navigator's screen mount; absent (a portal outside
    // any navigator) there's nothing to track, so we skip — the walker's
    // exact gating.
    if let Some(nav) = inject::<ScreenNav>() {
        let backend_c = backend.clone();
        let node_c = node.clone();
        let _visibility = effect(move || {
            let hidden = nav.active_route.get() != nav.route;
            backend_c.borrow_mut().set_portal_hidden(&node_c, hidden);
        });
    }

    // `release_portal` when the surrounding subtree drops (host's
    // open-state signal flipped, parent rebuilds, owner teardown) — the
    // old `PortalHandleCleanup` RAII guard. Release goes through the
    // try-borrow-else-microtask dance because this teardown can run
    // while the backend is already mutably borrowed (e.g. a virtualizer
    // release synchronously dropping row scopes that contain a portal —
    // the iOS "SIGABRT already-borrowed navigating away from a docs
    // page" crash the old `release_or_defer` exists to prevent).
    let backend_c = backend.clone();
    let node_c = node.clone();
    on_teardown(move || match backend_c.try_borrow_mut() {
        Ok(mut b) => b.release_portal(&node_c),
        Err(_) => {
            let backend2 = backend_c.clone();
            let node2 = node_c.clone();
            runtime_core::schedule_microtask(move || {
                if let Ok(mut b) = backend2.try_borrow_mut() {
                    b.release_portal(&node2);
                }
            });
        }
    });

    node
}

/// Register the portal handler (also serving the overlay compositions,
/// which lower to `PortalPrim`). Called by
/// [`register_builtins`](crate::handlers::register_builtins); available
/// separately for backends assembling a custom registry.
pub fn register_portal<H>(registry: &mut Registry<H>)
where
    H: PortalOps + StyleServices + 'static,
{
    registry.register::<PrimCell<PortalPrim>, _>(|cx, p, children| {
        mount_portal(cx, p.take(), children)
    });
}
