//! Code-splitting payload: `lazy` — the new-core carrier of the old
//! `Element::Lazy` (wasm-split chunk boundary).
//!
//! The payload shape mirrors `runtime_shared::primitives::lazy` with one
//! deliberate divergence: the loader future resolves to a **body
//! thunk** (`FnOnce() -> Element`), not a constructed `Element`. On the
//! old core the chunk fn built its `Element` *inside* the loader future
//! and the walker re-entered the chunk's reactive scope around every
//! poll (`ScopedLoad`) so construction-time signals had an owner. The
//! new core stages every write through an ambient [`World`] — a future
//! poll on the executor has **no world entered**, so constructing the
//! element there would panic in `component_scope`'s collector. The
//! thunk defers construction to the mount handler's swap effect, which
//! runs with its world entered and wraps the call in
//! [`runtime_scene::component_scope`] — construction-time state is
//! collected and dies with the chunk subtree, the exact invariant
//! `ScopedLoad` existed to protect, with no per-poll scope re-entry.
//!
//! [`World`]: runtime_world::World

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::lazy::{LazyError, LazyState};
use runtime_scene::Element;

use crate::style_attach::StyleProp;

/// The chunk's deferred body: constructs the loaded component's
/// element. Produced by the `#[component(lazy)]` new-core emission
/// (`__lazy_body` returns one); invoked by the mount handler inside
/// the swap effect's `component_scope` (module docs).
pub type LazyBodyThunk = Box<dyn FnOnce() -> Element>;

/// Future that resolves to the chunk's body thunk, or an `Err` carrying
/// a human-readable failure message (web: fetch / dynamic-link
/// failure). The new-core twin of `runtime_shared::primitives::lazy::
/// LazyFuture`, thunk-flavored per the module docs.
pub type LazyFuture = Pin<Box<dyn Future<Output = Result<LazyBodyThunk, String>>>>;

/// Closure that begins loading the chunk and returns a future for the
/// result. Re-invoked on [`LazyError::retry`], so it lives behind `Fn`.
pub type LazyLoader = Box<dyn Fn() -> LazyFuture>;

/// The `lazy` primitive: a chunk boundary that shows `placeholder`
/// until the loader resolves, then swaps in the chunk's body (or the
/// `error` UI on failure). Mounted by `handlers::lazy::mount_lazy`.
pub struct LazyPrim {
    /// Robot/automation anchor (`test_id = …`) — present for builder
    /// parity; the old core never registered `Element::Lazy` in the
    /// robot registry, and neither does the new handler.
    pub test_id: Option<&'static str>,
    /// Begins the chunk load; see [`LazyLoader`].
    pub loader: LazyLoader,
    /// Lifecycle observer (`Loading` → `Rendered` / `Error`), same
    /// event contract as the old walker (`Loaded` is skipped there
    /// too).
    pub on_state: Option<Rc<dyn Fn(LazyState)>>,
    /// Loading UI, rebuilt on every retry. `Rc` (not `Box`) for the
    /// same reason as the old walker: retry re-mounts it.
    pub placeholder: Option<Rc<dyn Fn() -> Element>>,
    /// Error UI; receives the shared [`LazyError`] (message + retry).
    pub error: Option<Rc<dyn Fn(&LazyError) -> Element>>,
    /// Style for the container view (the "lazy-boundary wrapper div").
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
}
