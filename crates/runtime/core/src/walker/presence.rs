//! `Element::Presence` build path. Manages mount/unmount timing so
//! the child's enter/exit animations actually have a window to
//! play.
//!
//! State machine:
//!
//! - **Initially absent** (`present()` is false): the child isn't
//!   built. The placeholder sits empty.
//! - **Mounting (false → true)**: build the child in a fresh
//!   scope. If `enter` is set: apply `enter.state` synchronously
//!   (snap pre-paint), then schedule one animation frame later to
//!   apply the rest state with the enter transition.
//! - **Mounted, present remains true**: the effect re-runs on
//!   signal changes inside `present`, but if the bool didn't flip
//!   we leave everything alone.
//! - **Exiting (true → false)**: if `exit` is set, apply
//!   `exit.state` with the exit transition, schedule a timer for
//!   `exit.duration_ms` that drops the scope. If `exit` is None,
//!   drop the scope immediately.
//! - **Reversal (exiting → true)**: cancel the pending drop timer,
//!   re-apply rest state with the enter transition (so the in-
//!   flight animation reverses smoothly). The scope is reused.
//!
//! All scope storage + scheduled task storage is in `Rc<RefCell>`
//! fields shared between the outer effect and the per-frame timers.
//! Drop semantics: when the surrounding scope drops (e.g. parent
//! `when` rebuilds), our owned `child_scope` drops, which drops the
//! child's subtree; the `ScheduledTask` drops at the same time,
//! cancelling any in-flight timer.

use super::debug::time_backend_create;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::element::Element;
use crate::primitives;
use crate::reactive::{self, untrack, Effect};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    child: Box<dyn Fn() -> Element>,
    present: Box<dyn Fn() -> bool>,
    enter: Option<primitives::presence::PresenceAnim>,
    exit: Option<primitives::presence::PresenceAnim>,
    ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    let n = build_presence(backend, child, present, enter, exit, &a11y);
    if let Some(RefFill::Presence(fill)) = ref_fill {
        let handle = backend.borrow().make_presence_handle(&n);
        fill(handle);
    }
    n
}

fn build_presence<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    child_fn: Box<dyn Fn() -> Element>,
    present: Box<dyn Fn() -> bool>,
    enter: Option<primitives::presence::PresenceAnim>,
    exit: Option<primitives::presence::PresenceAnim>,
    a11y: &AccessibilityProps,
) -> B::Node {
    use primitives::presence::PresenceState;

    let placeholder =
        time_backend_create(pkind!(Presence), || backend.borrow_mut().create_view(a11y));

    // Shared state across the effect + scheduled tasks. `Rc<RefCell>`
    // so the outer Effect and the timer closures all reach the same
    // entry. `child_node` is `Option<Self::Node>` so we can tell
    // "currently mounted" apart from "absent."
    let child_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> = Rc::new(RefCell::new(None));
    let child_node: Rc<RefCell<Option<B::Node>>> = Rc::new(RefCell::new(None));
    let pending_exit: Rc<RefCell<Option<crate::scheduling::ScheduledTask>>> =
        Rc::new(RefCell::new(None));
    let pending_enter: Rc<RefCell<Option<crate::scheduling::ScheduledTask>>> =
        Rc::new(RefCell::new(None));
    let last_present: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));

    let backend_for_effect = backend.clone();
    let placeholder_for_effect = placeholder.clone();
    let child_scope_for_effect = child_scope.clone();
    let child_node_for_effect = child_node.clone();
    let pending_exit_for_effect = pending_exit.clone();
    let pending_enter_for_effect = pending_enter.clone();
    let last_present_for_effect = last_present.clone();

    let _e = Effect::new(move || {
        let want_present = present();
        let was_present = *last_present_for_effect.borrow();
        *last_present_for_effect.borrow_mut() = want_present;

        if want_present && !was_present {
            // ---- Mount (off → on) ----
            // Cancel any in-flight exit timer (shouldn't be one if
            // was_present == false, but be defensive against the
            // case where we mount-exit-mount within a single tick).
            *pending_exit_for_effect.borrow_mut() = None;

            // Build the child in a fresh nested scope. `untrack` so
            // inner setup signal reads don't subscribe back to this
            // effect — they wire their own per-node effects.
            let mut new_scope = Box::new(reactive::Scope::new());
            let built_node: Rc<RefCell<Option<B::Node>>> = Rc::new(RefCell::new(None));
            let built_node_inner = built_node.clone();
            let backend_inner = backend_for_effect.clone();
            let child_fn_call = || child_fn();
            untrack(|| {
                reactive::with_scope(&mut new_scope, || {
                    let prim = child_fn_call();
                    let node = super::build(&backend_inner, 0, prim);
                    *built_node_inner.borrow_mut() = Some(node);
                });
            });
            let node = built_node.borrow_mut().take().expect("presence child built");
            // Insert into the placeholder.
            let mut placeholder_mut = placeholder_for_effect.clone();
            backend_for_effect
                .borrow_mut()
                .insert(&mut placeholder_mut, node.clone());
            *child_scope_for_effect.borrow_mut() = Some(new_scope);
            *child_node_for_effect.borrow_mut() = Some(node.clone());

            // If `enter` is set, snap to the enter state pre-paint,
            // then schedule the animate-to-rest one frame later.
            if let Some(anim) = enter {
                backend_for_effect
                    .borrow_mut()
                    .apply_presence(&node, anim.state, None);
                // Schedule the resting-state apply. Holding the
                // ScheduledTask in `pending_enter` lets a quick
                // unmount cancel it (otherwise we'd race the timer
                // against a freshly-applied exit state).
                let backend_for_frame = backend_for_effect.clone();
                let pending_enter_for_clear = pending_enter_for_effect.clone();
                let task = crate::scheduling::after_animation_frame(move || {
                    backend_for_frame.borrow_mut().apply_presence(
                        &node,
                        PresenceState::rest(),
                        Some((anim.duration_ms, anim.easing)),
                    );
                    // Self-clear: once the frame fires, the task is
                    // spent. Drop our handle so subsequent state
                    // checks see `None`.
                    *pending_enter_for_clear.borrow_mut() = None;
                });
                *pending_enter_for_effect.borrow_mut() = Some(task);
            }
        } else if !want_present && was_present {
            // ---- Unmount (on → off) ----
            // Cancel any pending enter timer — the child shouldn't
            // animate toward "rest" if we're about to exit it.
            *pending_enter_for_effect.borrow_mut() = None;

            let node_opt = child_node_for_effect.borrow().clone();
            let node = match node_opt {
                Some(n) => n,
                None => return,
            };

            if let Some(anim) = exit {
                backend_for_effect.borrow_mut().apply_presence(
                    &node,
                    anim.state,
                    Some((anim.duration_ms, anim.easing)),
                );
                // Schedule scope drop after the animation completes.
                let child_scope_for_timer = child_scope_for_effect.clone();
                let child_node_for_timer = child_node_for_effect.clone();
                let backend_for_timer = backend_for_effect.clone();
                let placeholder_for_timer = placeholder_for_effect.clone();
                let pending_exit_for_clear = pending_exit_for_effect.clone();
                let task = crate::scheduling::after_ms(anim.duration_ms as i32, move || {
                    // Tear down the child: drop its scope (which
                    // frees every signal/effect/ref inside) and
                    // remove its node from the placeholder.
                    *child_scope_for_timer.borrow_mut() = None;
                    *child_node_for_timer.borrow_mut() = None;
                    backend_for_timer
                        .borrow_mut()
                        .clear_children(&placeholder_for_timer);
                    *pending_exit_for_clear.borrow_mut() = None;
                });
                *pending_exit_for_effect.borrow_mut() = Some(task);
            } else {
                // No exit animation — drop the scope immediately.
                *child_scope_for_effect.borrow_mut() = None;
                *child_node_for_effect.borrow_mut() = None;
                backend_for_effect
                    .borrow_mut()
                    .clear_children(&placeholder_for_effect);
            }
        } else if want_present && was_present {
            // ---- Reversal mid-exit ----
            // If a pending exit task is alive, the user just
            // flipped back to present *during* the exit animation.
            // Cancel the timer (dropping the task) and re-animate
            // toward rest from wherever the interpolation currently
            // is.
            if pending_exit_for_effect.borrow().is_some() {
                *pending_exit_for_effect.borrow_mut() = None;
                if let Some(anim) = enter {
                    if let Some(node) = child_node_for_effect.borrow().clone() {
                        backend_for_effect.borrow_mut().apply_presence(
                            &node,
                            PresenceState::rest(),
                            Some((anim.duration_ms, anim.easing)),
                        );
                    }
                } else if let Some(node) = child_node_for_effect.borrow().clone() {
                    // No enter animation declared — snap back to
                    // rest with no transition.
                    backend_for_effect
                        .borrow_mut()
                        .apply_presence(&node, PresenceState::rest(), None);
                }
            }
        }
    });

    placeholder
}
