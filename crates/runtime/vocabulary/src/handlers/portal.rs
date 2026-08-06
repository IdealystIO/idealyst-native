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
    // `on_dismiss` is held by native chrome (an Escape controller, a
    // backdrop tap target) that can outlive the portal's scope.
    let alive = crate::callback_guard::ScopeAlive::current();
    let dismiss_for_backend = alive.wrap0_opt(prim.on_dismiss.clone());
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
    //
    // The `is_alive` probe is the consumer-side half of the stale-context
    // fix (`realize_screen`'s owned `ctx_scope` is the other). `ScreenNav`
    // arrives by `inject`, so its `active_route` is a `Copy` handle from a
    // scope this portal has no relationship with, and `effect` runs its
    // body immediately — a dead handle here aborts the app during this
    // portal's own mount rather than at some later read. Provider-side
    // ownership should mean we never see one; probing anyway costs a
    // bounds+generation check at mount and downgrades any residual
    // provider bug from "app dies" to "this portal doesn't hide when its
    // screen goes inactive".
    if let Some(nav) = inject::<ScreenNav>().filter(|n| n.active_route.is_alive()) {
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
            runtime_shared::schedule_microtask(move || {
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
