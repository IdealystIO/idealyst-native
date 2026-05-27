//! `Primitive::Lazy` build path.
//!
//! Mounts the placeholder synchronously, then spawns an async task
//! that drives the loader. When the loader's future resolves with
//! the chunk's `Primitive`, we build it and replace the
//! placeholder's children with the chunk's content.
//!
//! - **Wasm**: the loader is `wasm-split`'s generated wrapper. Its
//!   future awaits the chunk fetch + the chunk's async fn before
//!   yielding the `Primitive`.
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
use crate::primitive::Primitive;
use crate::primitives::lazy::{LazyLoader, LazyState};
use crate::sources::StyleSource;
use std::cell::RefCell;
use std::rc::Rc;

#[allow(clippy::too_many_arguments)]
pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    loader: LazyLoader,
    on_state: Option<Rc<dyn Fn(LazyState)>>,
    placeholder: Option<Box<dyn Fn() -> Primitive>>,
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

    // Drive the loader inside an async task. The closure captures
    // the container node, the chunk slot, and the state callback.
    // On native the future resolves on first poll; on wasm the
    // `wasm-split` runtime drives the dynamic import + chunk
    // function invocation before the future yields a Primitive.
    //
    // Requires the `async-driver` feature on runtime-core. Without
    // it the chunk never loads (placeholder stays visible
    // indefinitely) — Lazy is an async-by-nature primitive, so we
    // don't pretend to support a non-async build cleanly. The
    // wrapper template enables async-driver unconditionally; this
    // gate is purely so the framework itself still compiles in
    // minimal configurations (the audit + ports may not enable it).
    #[cfg(feature = "async-driver")]
    {
        let backend_for_async = backend.clone();
        let container = n.clone();
        let chunk_slot = chunk_node.clone();
        let state_cb = on_state.clone();
        crate::driver::spawn_async(async move {
            let chunk_primitive = (loader)().await;
            let child_node = super::build_inner(&backend_for_async, chunk_primitive);
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
        let _ = (loader, &chunk_node);
    }

    // Hold the chunk_node slot for cleanup-on-scope-drop. The slot
    // is captured by an Effect that lives on the surrounding scope;
    // when the scope drops, the Effect's drop fires, dropping the
    // slot, which drops the chunk's `B::Node` (triggering whatever
    // release path the backend has registered for nodes — typically
    // a `Drop` impl on the node type itself).
    let _cleanup_effect = crate::reactive::Effect::new(move || {
        let _ = &chunk_node;
    });

    n
}
