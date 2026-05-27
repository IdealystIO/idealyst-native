//! `Primitive::Lazy` build path.
//!
//! **Status**: native dispatch works today via the thread-local
//! [`primitives::lazy::register`](crate::primitives::lazy::register)
//! registry. Web dispatch is still the placeholder fallback (full
//! dynamic-import handler lands in PR 6).
//!
//! Flow:
//!
//! - **Non-wasm**: look up the chunk in the thread-local registry.
//!   If registered, build the chunk's `Primitive` inline as a child
//!   of the container view and fire `Loaded` → `Rendered`. If not
//!   registered, fire `Error("chunk not registered")` and render
//!   the placeholder.
//! - **Wasm**: render the placeholder, fire `Loading` once. The
//!   chunk doesn't actually load yet (PR 6 wires the dynamic
//!   `import()` + `mount_chunk` lifecycle).

use super::debug::time_backend_create;
use super::style::attach_style;
use crate::accessibility::AccessibilityProps;
use crate::backend::Backend;
use crate::handles::RefFill;
use crate::primitive::Primitive;
use crate::primitives::lazy::{ChunkId, LazyBridge, LazyState};
use crate::sources::StyleSource;
use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::rc::Rc;

#[allow(clippy::too_many_arguments)]
pub(super) fn build<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    chunk: ChunkId,
    _type_id: TypeId,
    _type_name: &'static str,
    payload: Rc<dyn Any>,
    _bridge: LazyBridge,
    on_state: Option<Rc<dyn Fn(LazyState)>>,
    placeholder: Option<Box<dyn Fn() -> Primitive>>,
    style: Option<StyleSource>,
    _ref_fill: Option<RefFill>,
    a11y: AccessibilityProps,
) -> B::Node {
    // Container view that hosts either the chunk (native) or the
    // placeholder (web fallback today; PR 6 swaps it for the
    // dynamically loaded chunk).
    let mut n =
        time_backend_create(pkind!(Lazy), || backend.borrow_mut().create_view(&a11y));

    if let Some(s) = style {
        attach_style(backend, &n, s);
    }

    // --- Native dispatch ---------------------------------------------------
    //
    // The chunk crate is a normal cargo dep, so its `app(props)` is
    // reachable through the thread-local registry. We dispatch
    // synchronously and mount the resulting primitive inline. The
    // state callback fires `Loaded` → `Rendered` so author-driven
    // state UI behaves uniformly across native + web (web will fire
    // the same sequence asynchronously once PR 6 lands).
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(chunk_primitive) =
            crate::primitives::lazy::dispatch(chunk, payload.clone())
        {
            let child_node = super::build_inner(backend, chunk_primitive);
            backend.borrow_mut().insert(&mut n, child_node);
            if let Some(cb) = on_state.as_ref() {
                cb(LazyState::Loaded);
                cb(LazyState::Rendered);
            }
            return n;
        }
        // No thunk registered. Fire `Error` so author UI can surface
        // the misconfiguration. Render the placeholder as the
        // visible state. This is almost always a missed
        // `chunks::register(&mut backend)` call at bootstrap.
        if let Some(cb) = on_state.as_ref() {
            cb(LazyState::Error(format!(
                "no chunk registered for ChunkId(\"{chunk}\"); did you call \
                 `chunks::register(&mut backend)` (or the manual \
                 `runtime_core::primitives::lazy::register(...)`) at bootstrap?"
            )));
        }
        mount_placeholder(backend, &mut n, placeholder.as_deref());
        return n;
    }

    // --- Web fallback ------------------------------------------------------
    //
    // The dynamic-import handler ships in PR 6. Until then we render
    // the placeholder and fire `Loading` exactly once, so author
    // state UI sees a coherent (if stuck) lifecycle.
    #[cfg(target_arch = "wasm32")]
    {
        mount_placeholder(backend, &mut n, placeholder.as_deref());
        if let Some(cb) = on_state.as_ref() {
            cb(LazyState::Loading);
        }
        warn_once(chunk);
        n
    }
}

fn mount_placeholder<B: Backend + 'static>(
    backend: &Rc<RefCell<B>>,
    container: &mut B::Node,
    placeholder: Option<&dyn Fn() -> Primitive>,
) {
    if let Some(build) = placeholder {
        let child = build();
        let child_node = super::build_inner(backend, child);
        backend.borrow_mut().insert(container, child_node);
    }
}

/// Print a one-shot warning the first time a `Primitive::Lazy`
/// mounts on web. Prevents spam while still surfacing the "chunk
/// isn't actually loading" reality during development. Removed by
/// PR 6 when the web handler ships.
#[cfg(target_arch = "wasm32")]
fn warn_once(chunk: ChunkId) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if WARNED.swap(true, Ordering::Relaxed) {
        return;
    }
    let msg = format!(
        "[idealyst::lazy] Primitive::Lazy(chunk={chunk}) mounted on web. The web \
         chunk-loader is not yet implemented (lands in PR 6); the placeholder \
         will render but the chunk itself will not load. Native targets work today."
    );
    // Don't pull web-sys in here — keep runtime-core platform-free.
    eprintln!("{msg}");
}
