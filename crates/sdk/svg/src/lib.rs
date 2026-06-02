//! Third-party SVG renderer SDK for the idealyst framework.
//!
//! Provides an `Svg` primitive backed by the framework's
//! `Element::External` extension mechanism. Renders the same SVG
//! spec on every backend; the mechanism differs (native browser SVG
//! on web, resvg + tiny-skia on iOS/Android) but the output converges.
//!
//! # Usage
//!
//! ```ignore
//! // App bootstrap — one line per third-party SDK:
//! let mut backend = WebBackend::new("#app");
//! svg::register(&mut backend);
//!
//! // Inside a `ui!` block. `Svg` interpolates as an expression — the
//! // macro only knows the closed first-party set, so third-party
//! // primitives come in via `{ ... }` interpolation.
//! let markup = signal(LOGO_SVG.to_string());
//! let r: Ref<SvgHandle> = Ref::new();
//! ui! {
//!     View {
//!         { svg::Svg(SvgProps {
//!             markup: svg::markup(move || markup.get()),
//!             on_load: Some(Rc::new(|| log::info!("svg parsed"))),
//!             ..Default::default()
//!         }).bind(r.clone()) }
//!     }
//! }
//! // Read intrinsic dimensions from the parsed SVG:
//! let size = r.with(|h| h.intrinsic_size());
//! ```
//!
//! # Architecture
//!
//! - `Element::External` payload type is [`SvgProps`] — every prop
//!   (markup + callbacks) is owned by the SDK, not the framework.
//! - Per-backend `register(&mut backend)` impls live in cfg-gated
//!   `web` / `android` / `ios` modules below. Each one calls
//!   `backend.register_external::<SvgProps, _>(handler)` to install a
//!   builder closure keyed by `TypeId::of::<SvgProps>()`.
//! - Reactive markup flows through `Effect::new(...)` *inside* the
//!   per-backend handler. No framework-level `update_svg_markup`
//!   plumbing — the SDK owns its own update loop.
//! - `SvgHandle` is the typed ref-target. It carries a type-erased
//!   `Rc<dyn Any>` to the native node plus a `&'static dyn SvgOps`
//!   pointer that the active backend module exposes as a static.
#![deny(missing_docs)]

use runtime_core::{Bound, Element, IdealystSchema, Ref, RefFill};
use std::any::{Any, TypeId};
use std::rc::Rc;

// ============================================================================
// Public API surface
// ============================================================================

/// Author-supplied props for an `Svg` instance. Type-erased into a
/// `Element::External` payload at build time; the active backend's
/// registered handler reads the typed `Rc<SvgProps>` back out.
///
/// `markup` is reactive: the backend subscribes via `Effect::new(...)`
/// and re-renders whenever signals captured by the closure change.
/// Callbacks fire once per successful parse + raster (`on_load`) or
/// once per parse failure (`on_error`).
#[derive(IdealystSchema)]
pub struct SvgProps {
    /// Initial + reactive SVG markup. Use [`markup`] to coerce `&str`,
    /// `String`, or `Fn() -> String` into this closure shape.
    #[schema(constraint = "well-formed SVG document markup")]
    pub markup: Box<dyn Fn() -> String>,
    /// Fires after every successful render. On native backends this
    /// fires once per re-rasterization; on web it fires once per
    /// `innerHTML` assignment.
    ///
    /// `Rc` (not `Box`) because the framework owns the props via
    /// `Rc<SvgProps>` — handler closures clone the `Rc` into Effect
    /// bodies rather than moving the inner box.
    pub on_load: Option<Rc<dyn Fn()>>,
    /// Fires when the markup fails to parse. The payload is a
    /// human-readable description (resvg's parse error on native, an
    /// "innerHTML threw" stub on web). No callback fires on backends
    /// where the failure path can't be observed.
    pub on_error: Option<Rc<dyn Fn(String)>>,
}

impl Default for SvgProps {
    fn default() -> Self {
        Self {
            markup: Box::new(String::new),
            on_load: None,
            on_error: None,
        }
    }
}

/// Coerce `&str`, `String`, or `Fn() -> String` into the closure
/// shape [`SvgProps::markup`] expects. Static literals work without
/// thinking about closures:
///
/// ```ignore
/// svg::markup(LOGO_SVG)                   // static
/// svg::markup(move || sig.get())          // reactive
/// ```
pub fn markup<U: IntoSvgMarkup>(u: U) -> Box<dyn Fn() -> String> {
    u.into_svg_markup()
}

/// Coercion target for [`markup`]. Implemented for `&str`, `String`,
/// and any `Fn() -> String`, so the call site can pass static or
/// reactive markup interchangeably.
pub trait IntoSvgMarkup {
    /// Box the receiver into the `Fn() -> String` closure that
    /// [`SvgProps::markup`] stores.
    fn into_svg_markup(self) -> Box<dyn Fn() -> String>;
}

impl IntoSvgMarkup for &str {
    fn into_svg_markup(self) -> Box<dyn Fn() -> String> {
        let s = self.to_string();
        Box::new(move || s.clone())
    }
}

impl IntoSvgMarkup for String {
    fn into_svg_markup(self) -> Box<dyn Fn() -> String> {
        Box::new(move || self.clone())
    }
}

impl<F> IntoSvgMarkup for F
where
    F: Fn() -> String + 'static,
{
    fn into_svg_markup(self) -> Box<dyn Fn() -> String> {
        Box::new(self)
    }
}

// ============================================================================
// Handle + ops trait
// ============================================================================

/// Typed handle to a mounted `Svg`. Filled by `Ref::fill` after the
/// primitive mounts; users hold a `Ref<SvgHandle>` at the call site
/// and reach imperative ops via `r.with(|h| h.intrinsic_size())`.
///
/// The `ops` pointer is set by the active backend's module via the
/// `OPS` static (cfg-selected at the bottom of this file). The `node`
/// is type-erased — each backend's ops downcasts it internally to the
/// concrete native type (`web_sys::Element` / `GlobalRef` / `IosNode`).
#[derive(Clone)]
pub struct SvgHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn SvgOps,
}

impl SvgHandle {
    /// Wrap a type-erased native node + backend ops into a handle.
    /// Called by the `RefFill::External` closure that [`SvgBind::bind`]
    /// installs; user code receives the handle through `Ref::with`.
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn SvgOps) -> Self {
        Self { node, ops }
    }

    /// The SVG's natural pixel dimensions, as declared by its viewBox
    /// (or `width`/`height` attributes if no viewBox is present).
    /// Returns `None` until the first successful render — call this
    /// from `on_load` if you need the value synchronously after mount.
    pub fn intrinsic_size(&self) -> Option<(f32, f32)> {
        self.ops.intrinsic_size(&*self.node)
    }
}

/// Imperative-ops dispatch. Implementations live in each cfg-gated
/// backend module and downcast `node` to their concrete native type.
/// Defaults all return `None` / no-op so a backend that hasn't wired
/// a particular op degrades silently rather than panicking.
///
/// `Sync` bound: the trait object lives in a `static OPS: &dyn SvgOps`
/// slot per backend module, which Rust requires to be `Sync`. The ZST
/// impls each backend ships are trivially `Sync`.
pub trait SvgOps: Sync {
    /// The SVG's natural `(width, height)` in pixels once parsed, or
    /// `None` before the first successful render. Default returns
    /// `None`.
    fn intrinsic_size(&self, _node: &dyn Any) -> Option<(f32, f32)> {
        None
    }
}

/// Fallback ops used on targets with no `Svg` impl. Every method is a
/// no-op or returns `None`; user code keeps compiling and the
/// framework's `External` placeholder is what renders at runtime.
pub struct UnsupportedOps;
impl SvgOps for UnsupportedOps {}

// ============================================================================
// Constructor + bind
// ============================================================================

/// Build an `Svg` primitive. Returns a typed `Bound<SvgHandle>` so
/// `.bind(...)` is type-checked against `Ref<SvgHandle>`.
///
/// PascalCase intentionally — matches the visual cadence of first-
/// party primitives (`View`, `Image`) inside a `ui!` block.
/// Interpolate as `{ svg::Svg(SvgProps { .. }) }`.
#[allow(non_snake_case)]
pub fn Svg(props: SvgProps) -> Bound<SvgHandle> {
    Bound::new(Element::External {
        type_id: TypeId::of::<SvgProps>(),
        type_name: std::any::type_name::<SvgProps>(),
        payload: Rc::new(props) as Rc<dyn Any>,
        children: Vec::new(),
        style: None,
        ref_fill: None,
        accessibility: runtime_core::accessibility::AccessibilityProps::default(),
    })
}

/// Adds `.bind(r)` to `Bound<SvgHandle>` via an extension trait — the
/// orphan rule blocks an inherent `impl Bound<SvgHandle>` here, since
/// `Bound` is foreign. Bring this trait into scope to use the builder
/// `.bind(...)` on the value [`Svg`] returns.
pub trait SvgBind {
    /// Bind a `Ref<SvgHandle>` for imperative access. At mount time the
    /// framework calls the `RefFill::External` closure with the
    /// type-erased native node; we wrap it in an `SvgHandle` using the
    /// cfg-selected backend's `OPS` static and fill the ref.
    fn bind(self, r: Ref<SvgHandle>) -> Self;
}

impl SvgBind for Bound<SvgHandle> {
    fn bind(mut self, r: Ref<SvgHandle>) -> Self {
        if let Element::External { ref_fill, .. } = self.primitive_mut() {
            *ref_fill = Some(RefFill::External(Box::new(move |node_any| {
                r.fill(SvgHandle::new(node_any, OPS));
            })));
        }
        self
    }
}

/// One-stop import: `use svg::prelude::*;` brings in the constructor,
/// props struct, handle type, the `.bind(...)` extension trait, and
/// the `markup(...)` coercion helper.
pub mod prelude {
    pub use super::{markup, Svg, SvgBind, SvgHandle, SvgProps};
}

// ============================================================================
// Shared tree walker (native backends only)
// ============================================================================

// The walker translates a parsed `usvg::Tree` into trait-driven calls
// against per-backend native vector primitives. Only the iOS and
// Android backends consume it; the web backend hands markup directly
// to the browser's SVG renderer and macOS / other hosts use the
// no-op fallback. Gating on the consumer set (not just
// `not(wasm32)`) keeps the unused-code warning quiet on macOS host
// builds.
#[cfg(any(target_os = "ios", target_os = "android"))]
pub(crate) mod tree_walker;

// ============================================================================
// Backend selector
// ============================================================================

// Each platform module exposes:
//   - `pub fn register(backend: &mut <ConcreteBackend>)`
//   - `pub(crate) static OPS: &dyn SvgOps`
// Only one is compiled per target via cfg; the umbrella re-exports
// `register` from whichever module matches. On unsupported targets
// the fallback below keeps user code compiling — the framework's
// External placeholder is what renders at runtime.

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::register;
#[cfg(target_arch = "wasm32")]
static OPS: &dyn SvgOps = web::OPS;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub use android::register;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
static OPS: &dyn SvgOps = android::OPS;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub use ios::register;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
static OPS: &dyn SvgOps = ios::OPS;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
)))]
mod fallback {
    use runtime_core::Backend;
    /// No-op register for unsupported targets. User code calls this
    /// unconditionally; the framework's External placeholder shows up
    /// at runtime to make the missing binding obvious.
    pub fn register<B: Backend>(_backend: &mut B) {}
}

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
)))]
pub use fallback::register;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
)))]
static OPS: &dyn SvgOps = &UnsupportedOps;
