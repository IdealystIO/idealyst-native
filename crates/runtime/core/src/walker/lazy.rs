//! `Element::Lazy` build path.
//!
//! Mounts the placeholder synchronously, then spawns an async task
//! that drives the loader. When the loader's future resolves with
//! the chunk's `Element`, we build it and replace the
//! placeholder's children with the chunk's content.
//!
//! - **Wasm**: the loader is `wasm-split`'s generated wrapper. Its
//!   future awaits the chunk fetch + the chunk's async fn before
//!   yielding the `Element`.
//! - **Native**: the loader's future resolves synchronously on
//!   first poll because the chunk's async fn is just a regular
//!   async function compiled into the same binary.
//!
//! The on_state callback fires `Loading` synchronously on mount,
//! then `Rendered` when the chunk's primitive is built (or `Error`
//! if the load fails). `Loaded` is skipped — the gap between fetch
//! completion and primitive resolution is below the resolution of
//! a human-observable transition.

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::element::Element;
use crate::primitives::lazy::{LazyLoader, LazyState};
use crate::reactive;
use crate::sources::StyleSource;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Dropped when the surrounding scope tears down (it is captured by the
/// `_cleanup_effect`, whose arena slot the surrounding scope owns). Its
/// `Drop` cancels any in-flight load and tears the chunk scope down at the
/// *right moment* — parent teardown — instead of whenever a still-pending
/// load happens to resolve.
///
/// Without the cancel flag, a load that resolves *after* the parent
/// unmounted would call `build_inner` against an orphaned scope and
/// `insert` into a detached container (stale-mount / use-after-teardown).
/// The async continuation checks `cancelled` after its await and bails.
struct LazyCancelGuard {
    cancelled: Rc<Cell<bool>>,
    chunk_scope: Rc<RefCell<Option<Box<reactive::Scope>>>>,
}

impl Drop for LazyCancelGuard {
    fn drop(&mut self) {
        // Signal in-flight loads to abandon their post-await work.
        self.cancelled.set(true);
        // Drop the chunk's reactive scope now so its cleanup effects (e.g.
        // `release_graphics`) run at teardown rather than at late resolution.
        // Taking the `Option` also makes the future's `as_mut()` fail closed.
        let scope = self.chunk_scope.borrow_mut().take();
        drop(scope);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    loader: LazyLoader,
    on_state: Option<Rc<dyn Fn(LazyState)>>,
    placeholder: Option<Box<dyn Fn() -> Element>>,
    style: Option<StyleSource>,
    _ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    // Container view that hosts the placeholder first, then the
    // chunk's content once the loader resolves.
    let mut n =
        time_backend_create(pkind!(Lazy), || backend.borrow_mut().create_view(&a11y));

    if let Some(s) = style {
        attach_style(backend, &n, s);
    }

    // Fire Loading synchronously so author UI sees a consistent
    // first event whether the loader is async (web) or resolves on
    // first poll (native).
    if let Some(cb) = on_state.as_ref() {
        cb(LazyState::Loading);
    }

    // Mount the placeholder as a child of the container.
    if let Some(build) = placeholder.as_ref() {
        let child = build();
        let child_node = super::build_inner(backend, child);
        backend.borrow_mut().insert(&mut n, child_node);
    }

    // Track the chunk's mounted node so we can release it on scope
    // drop — the surrounding `Effect` adopts the slot's RAII via
    // capture, so when the parent scope drops, the chunk's backend
    // node releases through the standard cleanup path.
    let chunk_node: Rc<RefCell<Option<B::Node>>> = Rc::new(RefCell::new(None));

    // The chunk's reactive state (Switch/When/Graphics cleanup
    // effects, signals etc.) needs a scope to live in — otherwise
    // every Effect::new called while walking the chunk's primitive
    // tree has `owns = true`, drops immediately at the end of the
    // building function, and cascades a teardown of anything it
    // owned (the canonical symptom: a Graphics primitive inside the
    // chunk gets created, its cleanup Effect is rootless, drop runs
    // before the canvas's first rAF, the rAF then bails because the
    // instance is already released → blank canvas).
    //
    // We synthesize a scope here, run `build_inner` inside it, and
    // tie its lifetime to `_cleanup_effect` below so the surrounding
    // scope's drop tears the chunk down at the right moment. Without
    // this the bug only bites lazy/wasm-split because the
    // non-lazy walker path is always already inside a host scope
    // (app root or `when`/`switch` branch); spawn_async's body is
    // run as a fresh JS task with no active scope.
    let chunk_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
        Rc::new(RefCell::new(Some(Box::new(reactive::Scope::new()))));

    // Set when the surrounding scope tears down (see `LazyCancelGuard`).
    // The async continuation reads it after its await to abandon a load
    // that resolved after the parent unmounted.
    let cancelled: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    // Drive the loader inside an async task. The closure captures
    // the container node, the chunk slot, and the state callback.
    // On native the future resolves on first poll; on wasm the
    // `wasm-split` runtime drives the dynamic import + chunk
    // function invocation before the future yields a Element.
    //
    // Requires the `async-driver` feature on runtime-core. Without
    // it the chunk never loads (placeholder stays visible
    // indefinitely) — Lazy is an async-by-nature primitive, so we
    // don't pretend to support a non-async build cleanly. The
    // wrapper template enables async-driver unconditionally; this
    // gate is purely so the framework itself still compiles in
    // minimal configurations (the audit + ports may not enable it).
    // SSR (headless) keeps the placeholder: a one-shot server render
    // can't paint lazy content (GPU canvas, etc.), and on the native SSR
    // binary the loader would resolve synchronously on first poll and
    // swap the body in — making the server HTML diverge from the client's
    // placeholder, which hydration must then remount. `renders_lazy_chunks()`
    // is `false` only on SSR, so skipping the loader there leaves the
    // `.placeholder(…)` as the server's output; the live client hydrates
    // that placeholder and then loads the real chunk.
    #[cfg(feature = "async-driver")]
    if backend.borrow().renders_lazy_chunks()
    {
        let backend_for_async = backend.clone();
        let container = n.clone();
        let chunk_slot = chunk_node.clone();
        let chunk_scope_for_async = chunk_scope.clone();
        let cancelled_for_async = cancelled.clone();
        let state_cb = on_state.clone();
        crate::driver::spawn_async(async move {
            let chunk_primitive = (loader)().await;
            // The surrounding scope may have torn down while we awaited the
            // load (web: a real async chunk fetch). If so, the chunk scope is
            // gone and the container is detached — building/inserting now
            // would mount into a dead tree. Bail before touching either.
            if cancelled_for_async.get() {
                return;
            }
            let child_node = {
                let mut scope_borrow = chunk_scope_for_async.borrow_mut();
                // Fail closed if the scope was taken out from under us by a
                // teardown that raced the cancel check above.
                let Some(scope) = scope_borrow.as_mut() else {
                    return;
                };
                reactive::with_scope(scope.as_mut(), || {
                    super::build_inner(&backend_for_async, chunk_primitive)
                })
            };
            // Clear the placeholder + insert the chunk's content.
            // The container stays; only its children swap.
            {
                let mut be = backend_for_async.borrow_mut();
                be.clear_children(&container);
            }
            {
                let mut be = backend_for_async.borrow_mut();
                let mut container_mut = container.clone();
                be.insert(&mut container_mut, child_node.clone());
            }
            *chunk_slot.borrow_mut() = Some(child_node);
            if let Some(cb) = state_cb.as_ref() {
                cb(LazyState::Rendered);
            }
        });
    }
    #[cfg(not(feature = "async-driver"))]
    {
        // Suppress unused warnings; the loader is dropped (chunk
        // never loads) and Rendered is never fired.
        let _ = (loader, &chunk_node, &chunk_scope, &cancelled);
    }

    // Hold the chunk_node slot + a cancel guard for
    // cleanup-on-surrounding-scope-drop. When the surrounding scope drops,
    // this Effect's slot is freed, its closure drops, and with it the
    // `LazyCancelGuard` — which cancels any in-flight load and tears the
    // chunk's `Scope` down (running every cleanup the chunk registered,
    // e.g. `release_graphics`). Dropping `chunk_node` releases the chunk's
    // backend node through the standard path.
    let cancel_guard = LazyCancelGuard { cancelled, chunk_scope };
    let _cleanup_effect = crate::reactive::Effect::new(move || {
        let _ = &chunk_node;
        let _ = &cancel_guard;
    });

    n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: when the surrounding scope tears down, the cancel guard
    /// (dropped with the cleanup effect's closure) must both flip the
    /// cancellation flag and drop the chunk scope. The async continuation
    /// reads that flag after its await to abandon a late-resolving load
    /// instead of building into the orphaned scope / detached container.
    ///
    /// A full end-to-end async-teardown test would need a backend, an
    /// installed async executor, and a manually-resolved future to
    /// deterministically interleave teardown with resolution — none of
    /// which are reachable at this layer. This exercises the exact drop
    /// mechanism the fix relies on.
    #[test]
    fn cancel_guard_cancels_and_drops_scope_on_teardown() {
        let cancelled: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let chunk_scope: Rc<RefCell<Option<Box<reactive::Scope>>>> =
            Rc::new(RefCell::new(Some(Box::new(reactive::Scope::new()))));

        {
            let _guard = LazyCancelGuard {
                cancelled: cancelled.clone(),
                chunk_scope: chunk_scope.clone(),
            };
            assert!(!cancelled.get(), "not cancelled while the guard is live");
            assert!(chunk_scope.borrow().is_some(), "chunk scope live while the guard is live");
        }

        assert!(cancelled.get(), "teardown must cancel any in-flight load");
        assert!(
            chunk_scope.borrow().is_none(),
            "teardown must drop the chunk scope so its cleanups run at the right moment"
        );
    }
}
