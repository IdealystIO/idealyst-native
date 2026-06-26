//! `Element::When` and `Element::Switch` build paths, both the
//! closure-driven Effect form and the lazy-slot-capture declarative
//! form generator backends consume.
//!
//! [`build_when`] is the dispatcher invoked by the walker dispatcher
//! for `Element::When`; [`build_switch`] is the same role for
//! `Element::Switch`. They pick between the closure path and the
//! declarative path based on the backend's
//! `supports_lazy_slot_capture` capability plus whether the input
//! `Derived` carries structured metadata.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::backend::Backend;
use crate::element::Element;
use crate::reactive::{self, untrack, Effect};
use crate::scheduling::schedule_microtask;
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn build_when<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    cond: crate::derive::Derived<bool>,
    then: Box<dyn Fn() -> Element>,
    otherwise: Box<dyn Fn() -> Element>,
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
    arms: Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Element>)>,
    default: Box<dyn Fn() -> Element>,
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
    then: Box<dyn Fn() -> Element>,
    otherwise: Box<dyn Fn() -> Element>,
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

    // Last boolean the branch was built for. Rebuild ONLY when `cond()`
    // actually flips — mirroring `build_switch_closure`'s `last_key` dedup.
    // The Effect re-runs on every signal `cond` reads, but a predicate may
    // read extra signals beyond the boolean (e.g. a `version` tick used to
    // re-flip the icon after a toggle). Without this guard such a tick would
    // tear down + rebuild the branch while the boolean is unchanged —
    // recreating the subtree's gesture nodes and dropping any in-flight
    // press. `None` = not yet built (first fire always proceeds).
    let last_active: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
    let last_active_for_effect = last_active.clone();

    // Capture the ambient navigator context ONCE, synchronously, while
    // the screen's `AmbientNavGuard`/`ScreenStateGuard`/`ScreenRouteGuard`
    // are still on the stack. Re-established around every rebuild below so
    // a `link` (or anything reading `ambient_navigator()`) rebuilt by a
    // signal change keeps the navigator it was born with. Without this the
    // rebuild fires after the screen build returned (guards dropped) and
    // the link captures `None`, silently no-op'ing. Weak nav ref inside —
    // see `AmbientNavContext`.
    let nav_ctx = crate::primitives::navigator::shared::capture_ambient_nav_context();

    let compute = cond.compute.clone();
    let _e = Effect::new(move || {
        let active = (compute)();
        // Dep changed but the boolean didn't → the active branch is already
        // mounted and correct; skip the teardown/rebuild. (First fire: `None`
        // → proceeds and seeds the cache.)
        if *last_active_for_effect.borrow() == Some(active) {
            return;
        }
        *last_active_for_effect.borrow_mut() = Some(active);
        let hydrating = backend_for_effect.borrow().is_hydrating();

        // Drop the previous branch's scope before building the new one,
        // freeing its signals + effects atomically. Skipped on the first
        // fire during hydration: there is no previous scope (it's None),
        // and we MUST NOT clear the SSR arm children — the build below
        // walks through them via the hydration cursor and adopts.
        if !hydrating {
            *branch_scope_for_effect.borrow_mut() = None;
            backend_for_effect
                .borrow_mut()
                .clear_children(&placeholder_for_effect);
        }

        // Build inside a fresh nested scope. `untrack` keeps inner setup
        // reads from subscribing to *this* outer effect — inner effects
        // subscribe themselves when they run. Under hydration the
        // walker's `create_*` adopt the anchor's SSR children; under
        // a normal mount they build fresh and we `insert` below.
        let mut new_scope = Box::new(reactive::Scope::new());
        untrack(|| {
            // Re-establish the ambient nav context for the duration of the
            // subtree build so links built inside capture the restored
            // navigator. Drops at the end of this closure.
            let _nav_restore = nav_ctx.enter();
            reactive::with_scope(&mut new_scope, || {
                let branch = if active { then() } else { otherwise() };
                let child_node = super::build(&backend_for_effect, 0, branch);
                let mut placeholder_mut = placeholder_for_effect.clone();
                // `insert` is a same-parent move under hydration (the
                // adopted node is already an anchor child) — keeping
                // the call shape identical between paths.
                backend_for_effect
                    .borrow_mut()
                    .insert(&mut placeholder_mut, child_node);
            });
        });
        *branch_scope_for_effect.borrow_mut() = Some(new_scope);
    });

    placeholder
}

/// Anchorless `Element::When` — splices the active branch's node DIRECTLY
/// into the real `parent` (no `create_reactive_anchor` wrapper), used when
/// the backend reports
/// [`supports_child_splice`](crate::Backend::supports_child_splice).
///
/// Why anchorless: web's reactive anchor is `display: contents`
/// (layout-transparent), but a native wrapper view is a real box that
/// AUTO-sizes to its in-flow children. When the active branch is
/// `position: Absolute` (an overlay), the wrapper collapses to 0×0 and the
/// absolute child never paints even though Taffy framed it correctly — the
/// "`when`-mounted box never appears on Android" bug. Splicing the branch
/// into the real parent instead gives in-flow branch content normal flow
/// (it pushes siblings, like web) AND absolute branch content the real
/// parent as its containing block (it lands at the same pixels web's
/// `display: contents` produces) — full behavioral convergence, no
/// per-case wrapper hack.
///
/// Each branch (`then`/`otherwise`) returns exactly one `Element`, so the
/// region contributes exactly one node. On every `cond()` change the
/// Effect `remove_child`s its prior node, drops the old branch scope
/// (freeing its signals/effects), builds the new branch in a fresh scope,
/// and `insert_at`s it at the stable `base_index`. Returns the region's
/// initial node count (1) so the caller advances its running child index
/// for trailing static siblings.
pub(super) fn build_when_spliced<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    parent: &B::Node,
    base_index: usize,
    cond: crate::derive::Derived<bool>,
    then: Box<dyn Fn() -> Element>,
    otherwise: Box<dyn Fn() -> Element>,
) -> usize {
    let parent = parent.clone();
    let backend_for_effect = backend.clone();

    // The active branch's scope + its inserted node, replaced atomically
    // on each toggle. `Rc<RefCell>` so the Effect can swap them.
    let branch_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> = Rc::new(RefCell::new(None));
    let current_node: Rc<RefCell<Option<B::Node>>> = Rc::new(RefCell::new(None));
    let branch_scope_for_effect = branch_scope.clone();
    let current_node_for_effect = current_node.clone();

    // Rebuild ONLY when `cond()` actually flips — see `build_when_closure`
    // for the full rationale. A predicate that reads extra signals (e.g. a
    // `version` tick) must not re-splice the branch while the boolean is
    // unchanged: that recreates the branch's gesture nodes mid-press and
    // drops the click. `None` = not yet built (first fire always proceeds).
    let last_active: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
    let last_active_for_effect = last_active.clone();

    // Capture the ambient navigator context ONCE (guards still on the
    // stack); re-establish it around each rebuild so a `link` inside a
    // reactively-remounted branch keeps its navigator. See
    // `build_when_closure` for the full rationale.
    let nav_ctx = crate::primitives::navigator::shared::capture_ambient_nav_context();

    let compute = cond.compute.clone();
    let _e = Effect::new(move || {
        let active = (compute)();
        // Dep changed but the boolean didn't → the spliced branch is already
        // mounted and correct; skip the re-splice. (First fire: `None` →
        // proceeds and seeds the cache.)
        if *last_active_for_effect.borrow() == Some(active) {
            return;
        }
        *last_active_for_effect.borrow_mut() = Some(active);

        // Unmount the prior branch: remove its node from the parent, then
        // drop its scope. Order matters — remove the native view before
        // freeing the scope so a reactive style/text effect in the old
        // subtree can't fire against a half-detached node.
        if let Some(old) = current_node_for_effect.borrow_mut().take() {
            backend_for_effect.borrow_mut().remove_child(&parent, &old);
        }
        *branch_scope_for_effect.borrow_mut() = None;

        // Build the new branch inside a fresh nested scope, then splice it
        // at the region's stable base index. `untrack` keeps inner setup
        // reads from subscribing to THIS outer effect.
        let mut new_scope = Box::new(reactive::Scope::new());
        untrack(|| {
            let _nav_restore = nav_ctx.enter();
            reactive::with_scope(&mut new_scope, || {
                let branch = if active { then() } else { otherwise() };
                let child_node = super::build(&backend_for_effect, 0, branch);
                let mut parent_mut = parent.clone();
                backend_for_effect.borrow_mut().insert_at(
                    &mut parent_mut,
                    child_node.clone(),
                    base_index,
                );
                *current_node_for_effect.borrow_mut() = Some(child_node);
            });
        });
        *branch_scope_for_effect.borrow_mut() = Some(new_scope);
    });

    // The Effect ran once synchronously, so exactly one branch node is now
    // spliced — this region contributes 1 to the parent's child index.
    1
}

/// Anchorless `Element::Switch` — the [`build_when_spliced`] analogue for a
/// multi-arm switch. Splices the active arm's node DIRECTLY into `parent` (no
/// `create_reactive_anchor` wrapper) on backends that report
/// [`supports_child_splice`](crate::Backend::supports_child_splice).
///
/// Why this exists: the anchored path wraps the active arm in a real native
/// box that AUTO-sizes to its in-flow child. Under a parent that centers its
/// cross axis (`align_items: center`), that wrapper hugs the arm's content and
/// centers it — so a full-width-intended arm (e.g. an idea-ui `Field`) collapses
/// to its content width instead of filling, while web's `display: contents`
/// anchor lets the same arm fill. The "password Field inside a `switch` rebuild
/// stayed icon-width on macOS/iOS/Android but filled on web" divergence. Splicing
/// the arm into the real parent removes the wrapper, so the arm participates in
/// the parent's layout exactly as web's transparent anchor does.
///
/// Mirrors the synchronous shape of [`build_when_spliced`] rather than
/// [`build_switch_closure`]'s deferred-microtask path: the splice path is
/// native-only (web takes the `display: contents` anchor, and only web hydrates),
/// so the hydration handling and microtask deferral are unnecessary here. Each
/// arm returns exactly one `Element`, so the region contributes one node.
///
/// Dedup matches `build_switch_closure`: an *opaque* discriminant (the typed
/// `switch()` builder, whose `compute` always returns `Null` and whose `default`
/// closure does the real typed dispatch) rebuilds on every fire; a non-opaque
/// (declarative, literal-key arms) discriminant skips the rebuild when the JSON
/// key is unchanged.
pub(super) fn build_switch_spliced<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    parent: &B::Node,
    base_index: usize,
    discriminant: crate::derive::Derived<crate::__serde_json::Value>,
    arms: Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Element>)>,
    default: Box<dyn Fn() -> Element>,
) -> usize {
    let parent = parent.clone();
    let backend_for_effect = backend.clone();

    let branch_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> = Rc::new(RefCell::new(None));
    let current_node: Rc<RefCell<Option<B::Node>>> = Rc::new(RefCell::new(None));
    let branch_scope_for_effect = branch_scope.clone();
    let current_node_for_effect = current_node.clone();

    // Re-key dedup. Opaque (closure-driven `switch()`) can't dedup on the
    // always-`Null` key, so it rebuilds whenever the scrutinee's signals fire
    // — same as `build_switch_closure`. `None` = not yet built.
    let last_key: Rc<RefCell<Option<crate::__serde_json::Value>>> = Rc::new(RefCell::new(None));
    let last_key_for_effect = last_key.clone();

    let nav_ctx = crate::primitives::navigator::shared::capture_ambient_nav_context();

    let opaque = discriminant.is_opaque();
    let compute = discriminant.compute.clone();
    let arms: Rc<Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Element>)>> = Rc::new(arms);
    let default: Rc<dyn Fn() -> Element> = default.into();

    let _e = Effect::new(move || {
        let new_key = (compute)();
        if !opaque {
            let same = last_key_for_effect
                .borrow()
                .as_ref()
                .map(|prev| prev == &new_key)
                .unwrap_or(false);
            if same {
                return;
            }
        }
        *last_key_for_effect.borrow_mut() = Some(new_key.clone());

        // Unmount the prior arm (native view first, then its scope) before
        // building the new one — same ordering rationale as build_when_spliced.
        if let Some(old) = current_node_for_effect.borrow_mut().take() {
            backend_for_effect.borrow_mut().remove_child(&parent, &old);
        }
        *branch_scope_for_effect.borrow_mut() = None;

        let mut new_scope = Box::new(reactive::Scope::new());
        untrack(|| {
            let _nav_restore = nav_ctx.enter();
            reactive::with_scope(&mut new_scope, || {
                // Empty `arms` (the typed `switch()` shape) → always `default()`,
                // which dispatches against the cached typed scrutinee. Declarative
                // arms → match the JSON key, else `default()`.
                let branch = arms
                    .iter()
                    .find(|(pat, _)| pat == &new_key)
                    .map(|(_, builder)| builder())
                    .unwrap_or_else(|| (default)());
                let child_node = super::build(&backend_for_effect, 0, branch);
                let mut parent_mut = parent.clone();
                backend_for_effect.borrow_mut().insert_at(
                    &mut parent_mut,
                    child_node.clone(),
                    base_index,
                );
                *current_node_for_effect.borrow_mut() = Some(child_node);
            });
        });
        *branch_scope_for_effect.borrow_mut() = Some(new_scope);
    });

    // The Effect ran once synchronously, so exactly one arm node is spliced.
    1
}

/// Build a `Element::When` for backends that opt into
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
    then: Box<dyn Fn() -> Element>,
    otherwise: Box<dyn Fn() -> Element>,
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

/// Build a `Element::Switch` for backends that opted into lazy
/// slot capture (Roku). Each arm's subtree is captured as a slot
/// (a self-contained command list); the backend stashes those
/// keyed by their root node id and the runtime plays / tears them
/// down on the device based on the discriminant's match.
fn build_switch_declarative<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    discriminant: crate::derive::Derived<crate::__serde_json::Value>,
    arms: Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Element>)>,
    default: Box<dyn Fn() -> Element>,
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

/// Build a `Element::Switch` via the closure-driven Effect path.
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
    arms: Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Element>)>,
    default: Box<dyn Fn() -> Element>,
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
    let arms: Rc<Vec<(crate::__serde_json::Value, Box<dyn Fn() -> Element>)>> = Rc::new(arms);
    let default: Rc<dyn Fn() -> Element> = default.into();

    // Capture the ambient navigator context ONCE, synchronously, while the
    // screen's guards are still on the stack. Re-established around the
    // deferred rebuild below so a reactively-remounted `link` keeps its
    // navigator (see `build_when_closure` for the full rationale). The
    // hydration INLINE build above runs during the initial pass while the
    // guards are still live, so it needs no restore.
    let nav_ctx = crate::primitives::navigator::shared::capture_ambient_nav_context();

    // Hydration: build the active arm INLINE so the walker descends
    // through the SSR arm's nodes and adopts them (the cursor advances
    // past the anchor subtree naturally). Without this, the deferred
    // microtask below runs AFTER the walker has already continued past
    // the switch — by which point the cursor is parked on the SSR
    // arm's first child, the next sibling tries to adopt it and tag-
    // mismatches, and the microtask itself then `clear_children`s the
    // SSR DOM and rebuilds from scratch. With it, branch_scope and
    // last_key are seeded so the Effect's first scheduled microtask
    // recognises "same key already mounted" and no-ops; subsequent
    // re-keys (theme toggle etc.) take the normal deferred path.
    if backend.borrow().is_hydrating() {
        let new_key = (compute)();
        let mut new_scope = Box::new(reactive::Scope::new());
        untrack(|| {
            reactive::with_scope(&mut new_scope, || {
                let branch = arms
                    .iter()
                    .find(|(pat, _)| pat == &new_key)
                    .map(|(_, builder)| builder())
                    .unwrap_or_else(|| (default)());
                // Walk the arm: each `create_*` calls `hydrate_next`,
                // adopting the SSR DOM under the anchor. `super::build`
                // returns the arm's root node — for adopted SSR, that
                // node is already a child of the anchor, so the explicit
                // `insert` below is a same-parent `append_child` (move
                // to end). Harmless for a single-child arm; matches the
                // deferred path's call shape.
                let child_node = super::build(&backend, 0, branch);
                let mut placeholder_mut = placeholder.clone();
                backend.borrow_mut().insert(&mut placeholder_mut, child_node);
            });
        });
        *branch_scope.borrow_mut() = Some(new_scope);
        *last_key.borrow_mut() = Some(new_key);
    }

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
        let nav_ctx_for_microtask = nav_ctx.clone();

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

            // Buffered first fire during hydration: the inline build
            // above already mounted the arm against the SSR DOM, so the
            // wipe-and-rebuild below would discard adopted nodes. After
            // `finish()` clears `hydrating`, subsequent re-keys reach
            // the normal path.
            if backend_for_microtask.borrow().is_hydrating() {
                return;
            }

            // Diagnostic timing for the Switch re-key path. With
            // `debug-stats` off these calls are no-ops and the optimizer
            // strips them entirely; with it on they accumulate into
            // `runtime_core::debug` phase counters so the host can
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
                // Re-establish the ambient nav context for the subtree
                // build (the screen's guards are long gone by the time
                // this deferred microtask fires).
                let _nav_restore = nav_ctx_for_microtask.enter();
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
