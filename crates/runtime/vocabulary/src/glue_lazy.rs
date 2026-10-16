//! `glue::primitives::lazy` — the `#[component(lazy)]` emission surface.
//!
//! The names and call shapes the macro output uses (`lazy_split`,
//! `LazyBuilder::{placeholder,loading,on_error,on_state,with_style}`, the
//! `LazyLoadingUi`/`LazyErrorUi` prop slots, `__into_handler`). The
//! loader future resolves to a **body thunk** ([`LazyBodyThunk`]), not a
//! constructed `Element` — see `prims::lazy`'s module docs for why
//! (construction must run under the world, inside the mount handler's
//! swap effect, not in a naked executor poll). The `#[component(lazy)]`
//! emission generates the thunk-returning `__lazy_body` behind a
//! `#[wasm_split]` boundary, which the post-link splitter classifies by
//! the `__wasm_split_00___<module>___00_…` export/import naming.
//!
//! `LazyState` / `LazyError` are the runtime-shared types (inert data +
//! callbacks).

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
pub use runtime_shared::primitives::lazy::{LazyError, LazyState};
use runtime_scene::{item, Element};

pub use crate::prims::lazy::{LazyBodyThunk, LazyFuture, LazyLoader};
use crate::prims::{LazyPrim, PrimCell};
use crate::style_attach::IntoStyleProp;

use crate::glue::IntoElement;

/// The loading-UI slot on a lazy component's props.
/// Wraps a `Fn() -> impl IntoElement` closure. Default: empty.
#[derive(Default, Clone)]
pub struct LazyLoadingUi(Option<Rc<dyn Fn() -> Element>>);

impl<F, R> From<F> for LazyLoadingUi
where
    F: Fn() -> R + 'static,
    R: IntoElement,
{
    fn from(f: F) -> Self {
        LazyLoadingUi(Some(Rc::new(move || f().into_element())))
    }
}

impl LazyLoadingUi {
    /// Framework-internal: unwrap the placeholder builder for the
    /// emission (the builder re-wraps in `Rc` for retry re-mounts).
    #[doc(hidden)]
    pub fn __into_handler(self) -> Option<Box<dyn Fn() -> Element>> {
        self.0.map(|rc| {
            let b: Box<dyn Fn() -> Element> = Box::new(move || rc());
            b
        })
    }
}

/// The error-UI slot on a lazy component's props.
/// Wraps a `Fn(&LazyError) -> impl IntoElement` closure. Default: the
/// load failure is logged and the loading UI stays visible.
#[derive(Default, Clone)]
pub struct LazyErrorUi(Option<Rc<dyn Fn(&LazyError) -> Element>>);

impl<F, R> From<F> for LazyErrorUi
where
    F: Fn(&LazyError) -> R + 'static,
    R: IntoElement,
{
    fn from(f: F) -> Self {
        LazyErrorUi(Some(Rc::new(move |e| f(e).into_element())))
    }
}

impl LazyErrorUi {
    /// Framework-internal: unwrap the error builder for the emission.
    #[doc(hidden)]
    pub fn __into_handler(self) -> Option<Rc<dyn Fn(&LazyError) -> Element>> {
        self.0
    }
}

/// The `style` slot on a lazy component's props: the style of the
/// boundary's CONTAINER view (the wrapper around the loading / body /
/// error state), not of the component's own root. Accepts anything a
/// builder's `.style(...)` does — `StyleRules`, `Rc<StyleRules>`, a
/// sheet application, a `Clone` closure — and REPLACES the default
/// fill (`runtime_shared::primitives::lazy::lazy_boundary_fill_rules`).
/// Default: unset, so the boundary lays out as a bounded, fillable flex
/// column like a navigator outlet.
///
/// Holds a `Clone`-able factory rather than a `StyleProp` (which is not
/// `Clone`: its dynamic form is a boxed closure) so `#[component(lazy,
/// retryable)]` can derive `Clone` on the props.
#[derive(Default, Clone)]
pub struct LazyBoundaryStyle(Option<Rc<dyn Fn() -> crate::style_attach::StyleProp>>);

impl<S> From<S> for LazyBoundaryStyle
where
    S: IntoStyleProp + Clone + 'static,
{
    fn from(style: S) -> Self {
        LazyBoundaryStyle(Some(Rc::new(move || style.clone().into_style_prop())))
    }
}

impl LazyBoundaryStyle {
    /// Framework-internal: materialize the style for the emission.
    #[doc(hidden)]
    pub fn __into_style_prop(self) -> Option<crate::style_attach::StyleProp> {
        self.0.map(|f| f())
    }
}

/// Builder produced by [`lazy_split`]. Constructed by the
/// `#[component(lazy)]` emission; public so the expansion (in user
/// crates) can reach it.
#[must_use]
pub struct LazyBuilder {
    prim: LazyPrim,
}

impl LazyBuilder {
    /// Subscribe to lifecycle transitions (`Loading` → `Rendered` /
    /// `Error`; `Loaded` is never reported).
    pub fn on_state<F>(mut self, f: F) -> Self
    where
        F: Fn(LazyState) + 'static,
    {
        self.prim.on_state = Some(Rc::new(f));
        self
    }

    /// Set the placeholder rendered while the chunk loads.
    pub fn placeholder<F>(mut self, build: F) -> Self
    where
        F: Fn() -> Element + 'static,
    {
        self.prim.placeholder = Some(Rc::new(build));
        self
    }

    /// Alias of [`placeholder`](Self::placeholder).
    pub fn loading<F>(self, build: F) -> Self
    where
        F: Fn() -> Element + 'static,
    {
        self.placeholder(build)
    }

    /// The UI shown when the chunk fails to load; receives a
    /// [`LazyError`] carrying the message and a `retry` handle.
    pub fn on_error<F>(mut self, build: F) -> Self
    where
        F: Fn(&LazyError) -> Element + 'static,
    {
        self.prim.error = Some(Rc::new(build));
        self
    }

    /// Attach a style to the container view, REPLACING the default fill
    /// (`lazy_boundary_fill_rules`) — one source of truth for the
    /// wrapper, like any other view.
    pub fn with_style<S: IntoStyleProp>(mut self, style: S) -> Self {
        self.prim.style = Some(style.into_style_prop());
        self
    }

    /// Attach accessibility props to the container.
    pub fn with_accessibility(mut self, a11y: AccessibilityProps) -> Self {
        self.prim.a11y = a11y;
        self
    }

    /// Robot/automation anchor on the container.
    pub fn test_id(mut self, id: &'static str) -> Self {
        self.prim.test_id = Some(id);
        self
    }
}

impl IntoElement for LazyBuilder {
    fn into_element(self) -> Element {
        item(PrimCell::new(self.prim), Vec::new())
    }
}

/// Construct a lazy boundary from a load closure — called by the
/// `#[component(lazy)]` emission, with a one-shot or retryable loader
/// whose future yields the body thunk.
pub fn lazy_split<F>(load: F) -> LazyBuilder
where
    F: Fn() -> LazyFuture + 'static,
{
    LazyBuilder {
        prim: LazyPrim {
            test_id: None,
            loader: Box::new(load),
            on_state: None,
            placeholder: None,
            error: None,
            style: None,
            a11y: AccessibilityProps::default(),
        },
    }
}
