//! `Primitive::When` and `Primitive::Switch` build paths, both the
//! closure-driven Effect form and the lazy-slot-capture declarative
//! form generator backends consume.
//!
//! [`build_when`] is the dispatcher invoked by the walker dispatcher
//! for `Primitive::When`; [`build_switch`] is the same role for
//! `Primitive::Switch`. They pick between the closure path and the
//! declarative path based on the backend's
//! `supports_lazy_slot_capture` capability plus whether the input
//! `Derived` carries structured metadata.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::backend::Backend;
use crate::primitive::Primitive;
use crate::reactive::{self, untrack, Effect};
use crate::scheduling::schedule_microtask;
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build_when<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    cond: crate::derive::Derived<bool>,
    then: Box<dyn Fn() -> Primitive>,
    otherwise: Box<dyn Fn() -> Primitive>,
    style: Option<StyleSource>,
) -> B::Node {
    // Two paths: declarative (the backend wants structured
    // metadata + both pre-built branches) or closure-based
    // (the existing Effect path that rebuilds the active
    // branch on signal change). The choice depends on (a)
    // whether `cond` carries structured metadata at all and
    // (b) whether the backend opts into the slot-capture
    // path for generator-style realization.
    let lazy = backend.borrow().supports_lazy_slot_capture();
    let n = if !cond.is_opaque() && lazy {
        build_when_declarative(backend, cond, then, otherwise)
    } else {
        build_when_closure(backend, cond, then, otherwise)
    };
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    n
}

pub(super) fn build_switch<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    discriminant: crate::derive::Derived<crate::__serde_json::Value>,
    arms: Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Primitive>)>,
    default: Box<dyn Fn() -> Primitive>,
    style: Option<StyleSource>,
) -> B::Node {
    let lazy = backend.borrow().supports_lazy_slot_capture();
    let n = if !discriminant.is_opaque() && lazy && !arms.is_empty() {
        build_switch_declarative(backend, discriminant, arms, default)
    } else {
        build_switch_closure(backend, discriminant, arms, default)
    };
    if let Some(s) = style {
        attach_style(backend, &n, s);
    }
    n
}

/// Renders a `When` primitive as a placeholder container whose subtree is
/// swapped each time `cond()` flips.
///
/// Lifecycle: the outer effect (registered with the surrounding scope)
/// reads `cond()` to track its dependencies. On every change it drops
/// the previous branch's nested `Scope` — freeing every signal and effect
/// in the old subtree atomically — and builds the new branch inside a
/// fresh nested scope.
fn build_when_closure<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    cond: crate::derive::Derived<bool>,
    then: Box<dyn Fn() -> Primitive>,
    otherwise: Box<dyn Fn() -> Primitive>,
) -> B::Node {
    let placeholder = time_backend_create(pkind!(View), || {
        backend.borrow_mut().create_reactive_anchor()
    });
    let backend_for_effect = backend.clone();
    let placeholder_for_effect = placeholder.clone();

    // The branch scope lives across effect re-runs. Rc<RefCell<Option<…>>>
    // so we can replace it atomically when the condition flips.
    let branch_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> = Rc::new(RefCell::new(None));
    let branch_scope_for_effect = branch_scope.clone();

    let compute = cond.compute.clone();
    let _e = Effect::new(move || {
        let active = (compute)();

        // Drop the previous branch's scope before building the new one,
        // freeing its signals + effects atomically.
        *branch_scope_for_effect.borrow_mut() = None;
        backend_for_effect
            .borrow_mut()
            .clear_children(&placeholder_for_effect);

        // Build inside a fresh nested scope. `untrack` keeps inner setup
        // reads from subscribing to *this* outer effect — inner effects
        // subscribe themselves when they run.
        let mut new_scope = Box::new(reactive::Scope::new());
        untrack(|| {
            reactive::with_scope(&mut new_scope, || {
                let branch = if active { then() } else { otherwise() };
                let child_node = super::build(&backend_for_effect, 0, branch);
                let mut placeholder_mut = placeholder_for_effect.clone();
                backend_for_effect
                    .borrow_mut()
                    .insert(&mut placeholder_mut, child_node);
            });
        });
        *branch_scope_for_effect.borrow_mut() = Some(new_scope);
    });

    placeholder
}

/// Build a `Primitive::When` for backends that opt into
/// declarative conditional rendering via
/// `handles_when_natively()`. Both branches are constructed
/// eagerly, both attached to the same anchor, and the binding
/// metadata is handed to the backend so it can ship the
/// "which-branch-is-active" decision over its wire format.
///
/// No `Effect` is set up here. The remote runtime is responsible
/// for evaluating the condition + toggling subtree visibility on
/// signal change. Closures + signal IDs both flow through the
/// binding so closure-driven backends still have everything they
/// need if they ever want to dual-path.
fn build_when_declarative<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    cond: crate::derive::Derived<bool>,
    then: Box<dyn Fn() -> Primitive>,
    otherwise: Box<dyn Fn() -> Primitive>,
) -> B::Node {
    let anchor = time_backend_create(pkind!(View), || {
        backend.borrow_mut().create_reactive_anchor()
    });

    // Backends taking the declarative path always pair this with
    // lazy slot capture today (Roku). The capture brackets each
    // branch's subtree build so the backend stashes the commands
    // separately for play/teardown on the device.
    backend.borrow_mut().begin_slot_capture();
    let then_node = super::build(backend, 0, then());
    backend.borrow_mut().end_slot_capture(&then_node);

    backend.borrow_mut().begin_slot_capture();
    let otherwise_node = super::build(backend, 0, otherwise());
    backend.borrow_mut().end_slot_capture(&otherwise_node);

    // Declare signals + the when binding to the backend.
    {
        let mut b = backend.borrow_mut();
        for (sid, val) in cond.inputs.iter().zip(cond.initial.iter()) {
            b.note_signal_initial(*sid, val);
        }
        b.note_when_binding(
            &anchor,
            &cond.inputs,
            cond.method,
            &then_node,
            &otherwise_node,
        );
    }

    anchor
}

/// Build a `Primitive::Switch` for backends that opted into lazy
/// slot capture (Roku). Each arm's subtree is captured as a slot
/// (a self-contained command list); the backend stashes those
/// keyed by their root node id and the runtime plays / tears them
/// down on the device based on the discriminant's match.
fn build_switch_declarative<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    discriminant: crate::derive::Derived<crate::__serde_json::Value>,
    arms: Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Primitive>)>,
    default: Box<dyn Fn() -> Primitive>,
) -> B::Node {
    let anchor = time_backend_create(pkind!(View), || {
        backend.borrow_mut().create_reactive_anchor()
    });

    // Capture each arm's commands as a slot.
    let mut arm_node_pairs: Vec<(crate::__serde_json::Value, B::Node)> =
        Vec::with_capacity(arms.len());
    for (value, builder) in arms.iter() {
        backend.borrow_mut().begin_slot_capture();
        let arm_node = super::build(backend, 0, builder());
        backend.borrow_mut().end_slot_capture(&arm_node);
        arm_node_pairs.push((value.clone(), arm_node));
    }
    backend.borrow_mut().begin_slot_capture();
    let default_node = super::build(backend, 0, default());
    backend.borrow_mut().end_slot_capture(&default_node);

    {
        let mut b = backend.borrow_mut();
        for (sid, val) in discriminant.inputs.iter().zip(discriminant.initial.iter()) {
            b.note_signal_initial(*sid, val);
        }
        b.note_switch_binding(
            &anchor,
            &discriminant.inputs,
            discriminant.method,
            &arm_node_pairs,
            &default_node,
        );
    }

    anchor
}

/// Build a `Primitive::Switch` via the closure-driven Effect path.
/// On each signal change inside `discriminant.compute`, the Effect
/// re-evaluates the discriminant, dedupes against the previously
/// seen JSON value, and (if changed) tears down the prior branch
/// scope and builds the new active arm. State inside the old
/// subtree is freed atomically. When `arms` is empty (the typed
/// `switch()` builder's degenerate shape) every fire just calls
/// `default()`, which internally dispatches against the typed
/// scrutinee.
fn build_switch_closure<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    discriminant: crate::derive::Derived<crate::__serde_json::Value>,
    arms: Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Primitive>)>,
    default: Box<dyn Fn() -> Primitive>,
) -> B::Node {
    let placeholder = time_backend_create(pkind!(View), || {
        backend.borrow_mut().create_reactive_anchor()
    });
    let backend_for_effect = backend.clone();
    let placeholder_for_effect = placeholder.clone();

    let branch_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> = Rc::new(RefCell::new(None));
    let last_key: Rc<RefCell<Option<crate::__serde_json::Value>>> = Rc::new(RefCell::new(None));
    let branch_scope_for_effect = branch_scope.clone();
    let last_key_for_effect = last_key.clone();

    // Opaque discriminant (closure-driven path) skips the JSON-key
    // dedup — its `compute` always returns `Null` and the
    // `default` closure does its own arm dispatch using a cached
    // typed scrutinee value, so we always need to re-fire after
    // every signal change.
    let opaque = discriminant.is_opaque();
    let compute = discriminant.compute.clone();
    let arms: Rc<Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Primitive>)>> = Rc::new(arms);
    let default: Rc<dyn Fn() -> Primitive> = default.into();

    let _e = Effect::new(move || {
        let new_key = (compute)();

        if !opaque {
            let same_as_last = last_key_for_effect
                .borrow()
                .as_ref()
                .map(|prev| prev == &new_key)
                .unwrap_or(false);
            if same_as_last {
                return;
            }
        }

        let placeholder_for_microtask = placeholder_for_effect.clone();
        let backend_for_microtask = backend_for_effect.clone();
        let branch_scope_for_microtask = branch_scope_for_effect.clone();
        let last_key_for_microtask = last_key_for_effect.clone();
        let arms_for_microtask = arms.clone();
        let default_for_microtask = default.clone();

        schedule_microtask(move || {
            if !opaque {
                let same_as_last = last_key_for_microtask
                    .borrow()
                    .as_ref()
                    .map(|prev| prev == &new_key)
                    .unwrap_or(false);
                if same_as_last {
                    return;
                }
            }

            // Diagnostic timing for the Switch re-key path. With
            // `debug-stats` off these calls are no-ops and the optimizer
            // strips them entirely; with it on they accumulate into
            // `framework_core::debug` phase counters so the host can
            // see where rebuild time is going.
            #[cfg(feature = "debug-stats")]
            let _t_scope_drop = crate::debug::now_micros();
            *branch_scope_for_microtask.borrow_mut() = None;
            #[cfg(feature = "debug-stats")]
            crate::debug::record_apply_phase(
                "switch_unmount_scope_drop",
                crate::debug::now_micros().saturating_sub(_t_scope_drop),
            );

            #[cfg(feature = "debug-stats")]
            let _t_clear = crate::debug::now_micros();
            backend_for_microtask
                .borrow_mut()
                .clear_children(&placeholder_for_microtask);
            #[cfg(feature = "debug-stats")]
            crate::debug::record_apply_phase(
                "switch_unmount_clear_children",
                crate::debug::now_micros().saturating_sub(_t_clear),
            );

            #[cfg(feature = "debug-stats")]
            let _t_build = crate::debug::now_micros();
            let mut new_scope = Box::new(reactive::Scope::new());
            untrack(|| {
                reactive::with_scope(&mut new_scope, || {
                    // Pick the matching arm by JSON equality, or
                    // fall through to default. Empty arms vec
                    // (the typed builder's degenerate shape)
                    // always falls through.
                    #[cfg(feature = "debug-stats")]
                    let _t_arm = crate::debug::now_micros();
                    let branch = arms_for_microtask
                        .iter()
                        .find(|(pat, _)| pat == &new_key)
                        .map(|(_, builder)| builder())
                        .unwrap_or_else(|| default_for_microtask());
                    #[cfg(feature = "debug-stats")]
                    crate::debug::record_apply_phase(
                        "switch_branch_arm_builder",
                        crate::debug::now_micros().saturating_sub(_t_arm),
                    );

                    #[cfg(feature = "debug-stats")]
                    let _t_tree = crate::debug::now_micros();
                    let child_node = super::build(&backend_for_microtask, 0, branch);
                    #[cfg(feature = "debug-stats")]
                    crate::debug::record_apply_phase(
                        "switch_branch_build_tree",
                        crate::debug::now_micros().saturating_sub(_t_tree),
                    );

                    #[cfg(feature = "debug-stats")]
                    let _t_attach = crate::debug::now_micros();
                    let mut placeholder_mut = placeholder_for_microtask.clone();
                    backend_for_microtask
                        .borrow_mut()
                        .insert(&mut placeholder_mut, child_node);
                    #[cfg(feature = "debug-stats")]
                    crate::debug::record_apply_phase(
                        "switch_branch_attach_to_placeholder",
                        crate::debug::now_micros().saturating_sub(_t_attach),
                    );
                });
            });
            #[cfg(feature = "debug-stats")]
            crate::debug::record_apply_phase(
                "switch_mount_build_branch",
                crate::debug::now_micros().saturating_sub(_t_build),
            );

            *branch_scope_for_microtask.borrow_mut() = Some(new_scope);
            *last_key_for_microtask.borrow_mut() = Some(new_key);
        });
    });

    placeholder
}
