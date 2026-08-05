//! `Lazy` primitive — wasm code-splitting boundaries.
//!
//! Declares a point in the UI tree where the subtree below is shipped
//! as a separate wasm chunk (web, via [`wasm-split`]) and inlined as
//! a regular function call on native targets. The chunk loads on first
//! mount; the author observes lifecycle via the `on_state` callback,
//! and a placeholder primitive is shown during the load.
//!
//! See `docs/proposals/lazy-primitive.md` for the full design.
//!
//! # Author API
//!
//! ```ignore
//! use runtime_core::{component, ui, Element};
//!
//! #[component(lazy)]
//! fn HeavyPanel(id: u32) -> Element {
//!     ui! { text { "loaded on demand from a chunk" } }
//! }
//!
//! ui! {
//!     text { "always loaded" }
//!     HeavyPanel(id = 42, loading = || ui! { text { "loading…" } })
//! }
//! ```
//!
//! `#[component(lazy)]` (alias `#[lazy_component]`) compiles the component's
//! body into a `#[wasm_split]`-annotated async function and emits a
//! [`Element::Lazy`] that drives the load+mount lifecycle; the props cross
//! the boundary as the loader's argument. (The deprecated `lazy! { … }`
//! block macro emits the same shape without props.)
//!
//! [`wasm-split`]: https://crates.io/crates/wasm-splitter

use std::rc::Rc;

// ---------------------------------------------------------------------------
// LazyState — lifecycle observable to author code via on_state.
// ---------------------------------------------------------------------------

/// Lifecycle phases of a `Element::Lazy`. Author code subscribes
/// via [`LazyBuilder::on_state`] to render its own loading / error
/// UI; the framework also mounts the [placeholder](LazyBuilder::placeholder)
/// during `Loading` / `Loaded` as an immediate fallback.
///
/// On native targets the chunk is compiled in and rendered inline —
/// the callback fires once with [`Rendered`](Self::Rendered) and
/// never observes [`Loading`](Self::Loading) or [`Loaded`](Self::Loaded).
#[derive(Clone, Debug)]
pub enum LazyState {
    /// Chunk fetch in flight. Web only — never observed on native.
    Loading,
    /// Chunk fetched and instantiated; the wrapper's async fn is
    /// being awaited. A brief window; many authors won't bother
    /// distinguishing this from `Loading`.
    Loaded,
    /// The chunk's `app()` returned and the subtree was mounted.
    Rendered,
    /// Fetch or invocation failed. The string is the underlying
    /// error, suitable for logging. Author decides whether to retry,
    /// fall back permanently, or surface to the user.
    Error(String),
}

impl LazyState {
    /// Convenience predicate for "show the placeholder" UI states.
    pub fn is_loading(&self) -> bool {
        matches!(self, LazyState::Loading | LazyState::Loaded)
    }

    /// Convenience predicate for "load failed" UI state.
    pub fn is_error(&self) -> bool {
        matches!(self, LazyState::Error(_))
    }
}

// ---------------------------------------------------------------------------
// LazyLoader — the load closure carried by Element::Lazy.
// ---------------------------------------------------------------------------


/// Failure handed to a lazy component's [`.error(..)`](LazyBuilder::on_error)
/// UI when its chunk can't load. Carries the failure `message` and a `retry`
/// handle that re-drives the load (re-running the loader under the chunk's
/// reactive scope). Cheap to clone — both fields are `Rc`.
///
/// Wire `retry` straight into a button:
/// ```ignore
/// .on_error(|e: &LazyError| ui! {
///     view {
///         text { format!("Couldn't load: {}", e.message()) }
///         Button(label = "Retry", on_press = e.retry())
///     }
/// })
/// ```
#[derive(Clone)]
pub struct LazyError {
    message: Rc<str>,
    retry: Rc<dyn Fn()>,
}

impl LazyError {
    /// Construct a `LazyError`. Framework-internal — the walker builds these
    /// from a loader's `Err(message)` plus the retry closure it owns. Author
    /// code receives them, never constructs them.
    #[doc(hidden)]
    pub fn __new(message: impl Into<Rc<str>>, retry: Rc<dyn Fn()>) -> Self {
        Self { message: message.into(), retry }
    }

    /// The failure detail (network error, missing chunk, panic in the body).
    /// Suitable for display or logging.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// A handle that re-attempts the load when called. Clone-cheap; pass it
    /// straight to an `on_press` / `on_click`:
    /// `Button(label = "Retry", on_press = err.retry())`.
    pub fn retry(&self) -> Rc<dyn Fn()> {
        self.retry.clone()
    }
}

impl std::fmt::Debug for LazyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The `retry` closure isn't printable; message is the useful part.
        f.debug_struct("LazyError").field("message", &self.message).finish_non_exhaustive()
    }
}


// ---------------------------------------------------------------------------
// Config slots for the `#[component(lazy)]` generated props.
// ---------------------------------------------------------------------------
//
// A lazy component's call site (`ui! { Profile(id = 5, loading = || …) }`) sets
// `loading` / `error` like any prop, and `ui!` coerces each value with
// `.into()`. These newtypes give that `.into()` a target: a blanket `From`
// over any `Fn() -> impl IntoElement` closure, so authors write a normal
// `ui!`-returning closure and the slot wraps + `IntoElement`-coerces it. The
// `#[component(lazy)]` macro generates these fields; author code never names
// the types directly.







// ---------------------------------------------------------------------------
// LazyBuilder + lazy_split() constructor — author surface (via macro).
// ---------------------------------------------------------------------------





// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

