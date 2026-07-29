//! Animated-lifecycle handler: `presence` — the old `walker/presence.rs`
//! state machine re-expressed on the scene's structural contract (design
//! §4): the mount/unmount swap is a guarded [`dyn_keyed`] hole keyed on
//! `present()`, and the exit-animation window is the Dyn driver's
//! **retire hook** — the handler holds the outgoing [`Retired`] subtree
//! (nodes still attached), plays the exit transform, and detaches + drops
//! it in the `after_ms` task. "Hold the value" replaces the old "defer
//! the scope drop".
//!
//! # State machine (per the old walker's doc, with the one sanctioned
//! semantic change at the bottom)
//!
//! - **Initially absent** (`present()` false): the hole holds an empty
//!   fragment; the placeholder sits empty.
//! - **Mounting (false → true)**: the driver builds the child fresh
//!   (untracked, in its own scope). If `enter` is set, the handler's
//!   animation effect — created AFTER the hole, so it runs after the
//!   driver on every fire — snaps `enter.state` pre-paint, then applies
//!   rest with the enter transition one animation frame later.
//! - **Exiting (true → false)**: the driver swaps to the empty branch
//!   and hands the outgoing subtree to the retire hook. With `exit` set,
//!   the hook applies `exit.state` with the exit transition and
//!   schedules detach-and-drop after `exit.duration_ms`; with no `exit`,
//!   it drops the scope and detaches immediately.
//! - **Re-present during exit**: the driver builds a FRESH child (enter
//!   animation applies) while the retired one finishes its exit and is
//!   removed by its own timer — a crossfade. This is the retire-hook
//!   model's deliberate semantic change from the old walker, which
//!   cancelled the exit and reused the outgoing scope; the design doc
//!   (§4) chose "hold the value" precisely so teardown stays
//!   drop-as-teardown with no deferred-scope machinery. Child-local
//!   state does NOT survive a mid-exit flicker on the new core.
//!
//! The exit timer is deliberately NOT cancelled on re-present: the
//! `ScheduledTask` closure owns the retired subtree AND its pending
//! `remove_child` calls — cancelling would drop the scope without ever
//! detaching the nodes, leaving a permanent ghost view in the tree.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_shared::primitives::presence::PresenceState;
use runtime_shared::scheduling::{after_animation_frame, after_ms, ScheduledTask};
use runtime_scene::{dyn_keyed, fragment, Element, LiveNode, MountCx, Registry, Retired};
use runtime_world::effect;

use crate::caps::{IntrospectionOps, PresenceOps};
use crate::prims::{PresencePrim, PrimCell};

/// Mount a `presence`.
pub fn mount_presence<H>(
    cx: &mut MountCx<'_, H>,
    prim: PresencePrim,
    _children: Vec<Element>,
) -> H::Node
where
    H: PresenceOps + IntrospectionOps,
{
    let backend = cx.backend().clone();
    // The placeholder is created bare and NEVER styled by the handler:
    // it must stay IN-FLOW so presence children participate in normal
    // layout (stacked toasts keep their gaps). Backends that need
    // absolute-fill behavior decide it per-child at insert time — see
    // the macOS presence in-flow placeholder fix; forcing
    // `absolute; inset: 0` here is exactly the bug that collapsed toast
    // stacks.
    let mut placeholder = backend
        .borrow_mut()
        .create_presence_placeholder(&prim.a11y);

    // Identity/robot registration on the PLACEHOLDER (the old core
    // registered `Element::Presence` itself): registered before the
    // hole realizes so the initially-present child links as its child.
    // Children built on later `present` flips run inside the driver
    // effect (empty parent stack) and surface via the snapshot orphan
    // rule, same as the old core.
    #[cfg(feature = "robot")]
    let _robot = crate::robot::register_mount(
        &backend,
        &placeholder,
        crate::robot::ElementKind::Presence,
        prim.test_id,
        None,
        None,
        crate::robot::MountActions::default(),
    );

    let present = prim.present;
    let enter = prim.enter;
    let exit = prim.exit;

    // Shared between the retire hook (exit) and the animation effect
    // (enter): holding a ScheduledTask keeps its callback alive;
    // overwriting with None cancels it.
    let pending_enter: Rc<RefCell<Option<ScheduledTask>>> = Rc::new(RefCell::new(None));
    let pending_exit: Rc<RefCell<Option<ScheduledTask>>> = Rc::new(RefCell::new(None));

    // The structural hole: guarded on the presence boolean (the
    // `last_active`-style dedup — a predicate reading extra signals must
    // not remount the child when only those extras changed), building
    // the child untracked in its own scope (the old walker's
    // `untrack_for_build` + fresh `Scope`).
    let child_fn = prim.child;
    let present_for_guard = present.clone();
    let hole = dyn_keyed(
        move || present_for_guard(),
        move |&on| if on { child_fn() } else { fragment(Vec::new()) },
    );

    // Exit half — the retire hook.
    let backend_for_retire = backend.clone();
    let pending_enter_for_retire = pending_enter.clone();
    let pending_exit_for_retire = pending_exit.clone();
    let hole = hole.with_retire(move |old| {
        let retired = old
            .downcast::<Retired<H::Node>>()
            .expect("presence retire hook: payload is Retired<H::Node>");
        let Retired {
            realized,
            parent,
            nodes,
        } = *retired;
        if nodes.is_empty() {
            // The retired subtree is the empty (absent) branch — a
            // re-present swap. Nothing to animate or detach.
            return;
        }
        // Cancel a pending enter — the child must not animate toward
        // rest while we're exiting it (old walker's exact guard).
        *pending_enter_for_retire.borrow_mut() = None;

        match exit {
            Some(anim) => {
                {
                    let mut b = backend_for_retire.borrow_mut();
                    for n in &nodes {
                        b.apply_presence(n, anim.state, Some((anim.duration_ms, anim.easing)));
                    }
                }
                // Detach + drop after the animation completes. Detach
                // BEFORE the scope drops (the spliced dispose rule: a
                // reactive effect in the exiting subtree must never
                // fire against a half-detached node).
                let backend_for_timer = backend_for_retire.clone();
                let pending_exit_clear = pending_exit_for_retire.clone();
                let task = after_ms(anim.duration_ms as i32, move || {
                    {
                        let mut b = backend_for_timer.borrow_mut();
                        for n in &nodes {
                            b.remove_child(&parent, n);
                        }
                    }
                    drop(realized);
                    *pending_exit_clear.borrow_mut() = None;
                });
                *pending_exit_for_retire.borrow_mut() = Some(task);
            }
            None => {
                // No exit animation — old walker order: drop the scope
                // immediately, then clear the placeholder.
                drop(realized);
                let mut b = backend_for_retire.borrow_mut();
                for n in &nodes {
                    b.remove_child(&parent, n);
                }
            }
        }
    });

    cx.realize_children_into(&mut placeholder, vec![hole]);

    // Reach the hole's live slot so the animation effect can find the
    // freshly built child on each mount.
    let watch = match cx.realized_children().last() {
        Some(LiveNode::Dyn(dyn_live)) => dyn_live.watch(),
        _ => unreachable!("presence realizes exactly one Dyn child"),
    };

    // Enter half — created AFTER the hole's driver effect, so on every
    // present() change it fires second (runtime-world notifies in
    // creation order) and observes the just-swapped-in child.
    let last_present = Rc::new(Cell::new(false));
    let backend_for_anim = backend.clone();
    let pending_enter_for_anim = pending_enter.clone();
    let _animation = effect(move || {
        let want = present();
        let was = last_present.replace(want);
        if !(want && !was) {
            return;
        }
        // ---- Mount (off → on) ----
        let Some(anim) = enter else { return };
        let nodes = watch.current_nodes();
        if nodes.is_empty() {
            return;
        }
        // Snap to the enter state pre-paint (no transition)…
        {
            let mut b = backend_for_anim.borrow_mut();
            for n in &nodes {
                b.apply_presence(n, anim.state, None);
            }
        }
        // …then animate to rest one frame later. Holding the task in
        // `pending_enter` lets a quick exit cancel it (otherwise the
        // rest-apply would race a freshly applied exit state).
        let backend_for_frame = backend_for_anim.clone();
        let pending_enter_clear = pending_enter_for_anim.clone();
        let task = after_animation_frame(move || {
            {
                let mut b = backend_for_frame.borrow_mut();
                for n in &nodes {
                    b.apply_presence(
                        n,
                        PresenceState::rest(),
                        Some((anim.duration_ms, anim.easing)),
                    );
                }
            }
            // Self-clear: once the frame fires, the task is spent.
            *pending_enter_clear.borrow_mut() = None;
        });
        *pending_enter_for_anim.borrow_mut() = Some(task);
    });

    if let Some(fill) = prim.ref_fill {
        let handle = backend.borrow().make_presence_handle(&placeholder);
        fill(handle);
    }

    placeholder
}

/// Register the presence handler. Called by
/// [`register_builtins`](crate::handlers::register_builtins); available
/// separately for backends assembling a custom registry.
pub fn register_presence<H>(registry: &mut Registry<H>)
where
    H: PresenceOps + IntrospectionOps + 'static,
{
    registry.register::<PrimCell<PresencePrim>, _>(|cx, p, children| {
        mount_presence(cx, p.take(), children)
    });
}
