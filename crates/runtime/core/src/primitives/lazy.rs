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

#[cfg(feature = "prim-lazy")]
use crate::accessibility::AccessibilityProps;
#[cfg(feature = "prim-lazy")]
use crate::builder::IntoElement;
#[cfg(feature = "prim-lazy")]
use crate::handles::RefFill;
use crate::element::Element;
#[cfg(feature = "prim-lazy")]
use crate::sources::{IntoStyleSource, StyleSource};
use std::future::Future;
use std::pin::Pin;
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

/// Future that resolves to the chunk's `Element`, or an `Err` carrying a
/// human-readable failure message when the chunk can't be loaded (web:
/// fetch / dynamic-link failure). Pinned + boxed so the framework can carry
/// many shapes through one slot.
///
/// The walker turns an `Err(message)` into a [`LazyError`] — attaching a
/// `retry` handle it owns — and hands it to the author's `.error(..)` UI.
/// The loader itself doesn't know how to retry; it only reports *what* went
/// wrong, so the same message shape works on every backend.
pub type LazyFuture = Pin<Box<dyn Future<Output = Result<Element, String>>>>;

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

/// Closure that begins loading the chunk and returns a future for
/// the result. On native this resolves synchronously (the chunk
/// crate's `app()` is compiled in); on wasm the future awaits the
/// `wasm-split` runtime's `LazyLoader::load` + `.call` sequence
/// before yielding the chunk's `Element`.
///
/// Generated by the `#[component(lazy)]` glue (and the deprecated
/// `lazy!` macro) — author code should never construct this by hand.
pub type LazyLoader = Box<dyn Fn() -> LazyFuture>;

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

/// The loading-UI slot on a lazy component's props. Wraps a `Fn() -> impl
/// IntoElement` closure. Default: empty (an empty view while loading).
#[cfg(feature = "prim-lazy")]
#[derive(Default, Clone)]
pub struct LazyLoadingUi(Option<Rc<dyn Fn() -> Element>>);

#[cfg(feature = "prim-lazy")]
impl<F, R> From<F> for LazyLoadingUi
where
    F: Fn() -> R + 'static,
    R: crate::builder::IntoElement,
{
    fn from(f: F) -> Self {
        LazyLoadingUi(Some(Rc::new(move || f().into_element())))
    }
}

#[cfg(feature = "prim-lazy")]
impl LazyLoadingUi {
    /// Framework-internal: unwrap the placeholder builder for the walker.
    #[doc(hidden)]
    pub fn __into_handler(self) -> Option<Box<dyn Fn() -> Element>> {
        self.0.map(|rc| {
            let b: Box<dyn Fn() -> Element> = Box::new(move || rc());
            b
        })
    }
}

/// The error-UI slot on a lazy component's props. Wraps a `Fn(&LazyError) ->
/// impl IntoElement` closure. Default: empty (the load failure is logged and
/// the loading UI stays visible).
#[cfg(feature = "prim-lazy")]
#[derive(Default, Clone)]
pub struct LazyErrorUi(Option<Rc<dyn Fn(&LazyError) -> Element>>);

#[cfg(feature = "prim-lazy")]
impl<F, R> From<F> for LazyErrorUi
where
    F: Fn(&LazyError) -> R + 'static,
    R: crate::builder::IntoElement,
{
    fn from(f: F) -> Self {
        LazyErrorUi(Some(Rc::new(move |e| f(e).into_element())))
    }
}

#[cfg(feature = "prim-lazy")]
impl LazyErrorUi {
    /// Framework-internal: unwrap the error builder for the walker.
    #[doc(hidden)]
    pub fn __into_handler(self) -> Option<Rc<dyn Fn(&LazyError) -> Element>> {
        self.0
    }
}

// ---------------------------------------------------------------------------
// LazyBuilder + lazy_split() constructor — author surface (via macro).
// ---------------------------------------------------------------------------

/// Builder produced by [`lazy_split`]. Use `.on_state(...)`,
/// `.placeholder(...)`, and `.with_style(...)` to configure, then
/// drop into a `ui!` block or call `.into_element()`.
///
/// Authors don't typically touch this directly — the `#[component(lazy)]`
/// glue constructs it. Public so the macro's emitted code (which expands
/// in user crates) can reach it.
#[cfg(feature = "prim-lazy")]
#[must_use]
pub struct LazyBuilder {
    loader: LazyLoader,
    on_state: Option<Rc<dyn Fn(LazyState)>>,
    placeholder: Option<Box<dyn Fn() -> Element>>,
    error: Option<Rc<dyn Fn(&LazyError) -> Element>>,
    style: Option<StyleSource>,
    ref_fill: Option<RefFill>,
    accessibility: AccessibilityProps,
}

#[cfg(feature = "prim-lazy")]
impl LazyBuilder {
    /// Subscribe to lifecycle transitions. Fires synchronously on
    /// each state change; the callback should be cheap (typically
    /// a single `Signal::set`). Use this to drive a placeholder /
    /// error UI elsewhere in the tree.
    pub fn on_state<F>(mut self, f: F) -> Self
    where
        F: Fn(LazyState) + 'static,
    {
        // Born batched — see `reactive::cycle`.
        self.on_state = Some(Rc::new(move |s: LazyState| crate::cycle(|| f(s))));
        self
    }

    /// Set the placeholder rendered while the chunk loads. On native
    /// the placeholder is never visible (the chunk mounts
    /// synchronously). Default: nothing (an empty view).
    pub fn placeholder<F>(mut self, build: F) -> Self
    where
        F: Fn() -> Element + 'static,
    {
        self.placeholder = Some(Box::new(build));
        self
    }

    /// The UI shown while the chunk is loading. Alias of
    /// [`placeholder`](Self::placeholder), named to read as one of the three
    /// lazy states (`loading` / `error` / ready). Use whichever reads best;
    /// the last one set wins.
    pub fn loading<F>(self, build: F) -> Self
    where
        F: Fn() -> Element + 'static,
    {
        self.placeholder(build)
    }

    /// The UI shown when the chunk fails to load (web: fetch / dynamic-link
    /// failure). The closure receives a [`LazyError`] carrying the failure
    /// message and a `retry` handle. Default: the load failure is logged and
    /// the loading UI stays visible.
    ///
    /// ```ignore
    /// .on_error(|e: &LazyError| ui! {
    ///     view {
    ///         text { format!("Couldn't load: {}", e.message()) }
    ///         Button(label = "Retry", on_press = e.retry())
    ///     }
    /// })
    /// ```
    pub fn on_error<F>(mut self, build: F) -> Self
    where
        F: Fn(&LazyError) -> Element + 'static,
    {
        self.error = Some(Rc::new(build));
        self
    }

    /// Attach a style to the container view (placeholder wrapper on
    /// web, mounted subtree's root on native).
    pub fn with_style<S: IntoStyleSource>(mut self, style: S) -> Self {
        self.style = Some(style.into_style_source());
        self
    }

    /// Attach accessibility props to the container.
    pub fn with_accessibility(mut self, a11y: AccessibilityProps) -> Self {
        self.accessibility = a11y;
        self
    }

    /// Alias of [`with_accessibility`](Self::with_accessibility), named
    /// to match the `Bound::accessibility` setter every other primitive
    /// exposes.
    pub fn accessibility(self, a11y: AccessibilityProps) -> Self {
        self.with_accessibility(a11y)
    }

    /// Set the spoken accessibility label on the container. See
    /// [`Bound::a11y_label`](crate::Bound::a11y_label).
    pub fn a11y_label(mut self, label: impl Into<String>) -> Self {
        self.accessibility.label = Some(label.into());
        self
    }

    /// Set the longer accessibility hint on the container.
    pub fn a11y_hint(mut self, hint: impl Into<String>) -> Self {
        self.accessibility.hint = Some(hint.into());
        self
    }

    /// Override the inferred accessibility role on the container.
    pub fn a11y_role(mut self, role: crate::accessibility::Role) -> Self {
        self.accessibility.role = Some(role);
        self
    }

    /// Hide the container from the accessibility tree.
    pub fn a11y_hidden(mut self, hidden: bool) -> Self {
        self.accessibility.hidden = hidden;
        self
    }

    /// Set the accessibility state flags on the container.
    pub fn a11y_traits(mut self, traits: crate::accessibility::AccessibilityTraits) -> Self {
        self.accessibility.traits = traits;
        self
    }

    /// Mark the container as a live region at the given priority.
    pub fn live_region(mut self, priority: crate::accessibility::LiveRegionPriority) -> Self {
        self.accessibility.live_region = Some(priority);
        self
    }
}

#[cfg(feature = "prim-lazy")]
impl IntoElement for LazyBuilder {
    fn into_element(self) -> Element {
        Element::Lazy {
            loader: self.loader,
            on_state: self.on_state,
            placeholder: self.placeholder,
            error: self.error,
            style: self.style,
            ref_fill: self.ref_fill,
            accessibility: self.accessibility,
        }
    }
}

/// Construct a lazy boundary from a load closure. Called by the
/// `#[component(lazy)]` glue — author code typically uses the macro,
/// not this function directly. Public so the macro's expansion (in
/// user crates) can reach it.
#[cfg(feature = "prim-lazy")]
pub fn lazy_split<F>(load: F) -> LazyBuilder
where
    F: Fn() -> LazyFuture + 'static,
{
    LazyBuilder {
        loader: Box::new(load),
        on_state: None,
        placeholder: None,
        error: None,
        style: None,
        ref_fill: None,
        accessibility: AccessibilityProps::default(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "prim-lazy"))]
mod tests {
    use super::*;
    use crate::{view, Element};

    fn make_loader() -> LazyLoader {
        Box::new(|| {
            Box::pin(async {
                // Simulate the chunk's `app()` returning a primitive.
                Ok(view(Vec::new()).into_element())
            })
        })
    }

    #[test]
    fn constructor_emits_lazy_variant() {
        let p = lazy_split(|| {
            Box::pin(async { Ok(view(Vec::new()).into_element()) })
        })
        .into_element();
        match p {
            Element::Lazy {
                on_state,
                placeholder,
                error,
                ..
            } => {
                assert!(on_state.is_none(), "default has no state observer");
                assert!(placeholder.is_none(), "default has no placeholder");
                assert!(error.is_none(), "default has no error handler");
            }
            _ => panic!("expected Element::Lazy"),
        }
    }

    #[test]
    fn builder_methods_compose() {
        let state_calls = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let counter = state_calls.clone();
        let p = lazy_split(|| Box::pin(async { Ok(view(Vec::new()).into_element()) }))
            .on_state(move |_s| counter.set(counter.get() + 1))
            .loading(|| view(Vec::new()).into_element())
            .on_error(|_e| view(Vec::new()).into_element())
            .into_element();
        let Element::Lazy {
            on_state,
            placeholder,
            error,
            ..
        } = p
        else {
            panic!("expected Element::Lazy");
        };
        assert!(on_state.is_some());
        assert!(placeholder.is_some(), "loading() aliases placeholder");
        assert!(error.is_some(), "on_error registers an error handler");
        on_state.unwrap()(LazyState::Loading);
        assert_eq!(state_calls.get(), 1);
    }

    #[test]
    fn lazy_state_predicates() {
        assert!(LazyState::Loading.is_loading());
        assert!(LazyState::Loaded.is_loading());
        assert!(!LazyState::Rendered.is_loading());
        assert!(!LazyState::Error("x".into()).is_loading());
        assert!(LazyState::Error("x".into()).is_error());
    }

    // Silence unused warning while keeping the helper around for
    // future tests of the load-and-mount integration.
    #[test]
    fn loader_helper_compiles() {
        let _ = make_loader();
    }

}
