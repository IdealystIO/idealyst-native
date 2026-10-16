//! Code-splitting payload: `lazy` (wasm-split chunk boundary).
//!
//! The payload shape follows `runtime_shared::primitives::lazy` with one
//! deliberate difference: the loader future resolves to a **body thunk**
//! (`FnOnce() -> Element`), not a constructed `Element`. Every write
//! stages through an ambient [`World`], and a future poll on the
//! executor has **no world entered**, so constructing the element there
//! would panic in `component_scope`'s collector. The thunk defers
//! construction to the mount handler's swap effect, which runs with its
//! world entered and wraps the call in
//! [`runtime_scene::component_scope`] — construction-time signals get an
//! owner and die with the chunk subtree, with no per-poll scope re-entry.
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
/// element. Produced by the `#[component(lazy)]` emission
/// (`__lazy_body` returns one); invoked by the mount handler inside
/// the swap effect's `component_scope` (module docs).
pub type LazyBodyThunk = Box<dyn FnOnce() -> Element>;

/// Future that resolves to the chunk's body thunk, or an `Err` carrying
/// a human-readable failure message (web: fetch / dynamic-link
/// failure). Thunk-flavored per the module docs.
pub type LazyFuture = Pin<Box<dyn Future<Output = Result<LazyBodyThunk, String>>>>;

/// Closure that begins loading the chunk and returns a future for the
/// result. Re-invoked on [`LazyError::retry`], so it lives behind `Fn`.
pub type LazyLoader = Box<dyn Fn() -> LazyFuture>;

/// The `lazy` primitive: a chunk boundary that shows `placeholder`
/// until the loader resolves, then swaps in the chunk's body (or the
/// `error` UI on failure). Mounted by `handlers::lazy::mount_lazy`.
pub struct LazyPrim {
    /// Robot/automation anchor (`test_id = …`) — accepted so every
    /// builder has the setter; the handler does not register the lazy
    /// boundary in the robot registry.
    pub test_id: Option<&'static str>,
    /// Begins the chunk load; see [`LazyLoader`].
    pub loader: LazyLoader,
    /// Lifecycle observer (`Loading` → `Rendered` / `Error`; `Loaded` is
    /// never reported).
    pub on_state: Option<Rc<dyn Fn(LazyState)>>,
    /// Loading UI, rebuilt on every retry. `Rc` (not `Box`): retry
    /// re-mounts it.
    pub placeholder: Option<Rc<dyn Fn() -> Element>>,
    /// Error UI; receives the shared [`LazyError`] (message + retry).
    pub error: Option<Rc<dyn Fn(&LazyError) -> Element>>,
    /// Style for the container view (the "lazy-boundary wrapper div").
    pub style: Option<StyleProp>,
    pub a11y: AccessibilityProps,
}
