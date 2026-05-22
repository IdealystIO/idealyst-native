//! Third-party `WebView` SDK for the idealyst framework.
//!
//! Provides a `WebView` primitive backed by the framework's
//! `Primitive::External` extension mechanism. Author-facing API
//! mirrors the framework's other reactive primitives — typed props,
//! `.bind(...)`-able handle, `.with_style(...)`.
//!
//! # Usage
//!
//! ```ignore
//! // In the app's bootstrap (one line per third-party SDK):
//! let mut backend = WebBackend::new("#app");
//! webview::register(&mut backend);
//!
//! // Inside a `ui!` block. `WebView` reads as a first-party primitive;
//! // because the macro only recognises the framework's closed set, it's
//! // interpolated as an expression.
//! let url = signal("https://example.com".to_string());
//! let wv: Ref<WebViewHandle> = Ref::new();
//! ui! {
//!     View {
//!         { webview::WebView(WebViewProps {
//!             url: webview::url(move || url.get()),
//!             on_load: Some(Box::new(|| log::info!("loaded"))),
//!             ..Default::default()
//!         }).bind(wv.clone()) }
//!     }
//! }
//! // Imperative ops at any later point:
//! wv.with(|h| h.reload());
//! ```
//!
//! # Architecture
//!
//! - The `Primitive::External` payload type is [`WebViewProps`] — all
//!   props (URL + callbacks) are owned by the SDK, not the framework.
//! - Per-backend `register(&mut backend)` impls live in cfg-gated
//!   `web` / `android` / `ios` modules below. Each one calls
//!   `backend.register_external::<WebViewProps, _>(handler)` to install
//!   a builder closure keyed by `TypeId::of::<WebViewProps>()`.
//! - `WebViewHandle` is the typed ref-target. It carries a type-erased
//!   `Rc<dyn Any>` to the native node + a `&'static dyn WebViewOps`
//!   pointer that the active backend module exposes as a static.
//!   `Bound<WebViewHandle>::bind` installs a `RefFill::External`
//!   closure that wraps the node any in a `WebViewHandle` using that
//!   static `OPS`.
//! - Reactive URL changes flow through `Effect::new(...)` *inside* the
//!   backend handler closure — the per-backend impl subscribes itself
//!   when it builds the native view. No framework-level
//!   `update_web_view_url` plumbing involved.

use framework_core::{Bound, Primitive, Ref, RefFill};
use std::any::{Any, TypeId};
use std::rc::Rc;

// ============================================================================
// Public API surface
// ============================================================================

/// Author-supplied props for a `WebView` instance. Owned by the SDK,
/// not the framework — the framework just type-erases this struct
/// behind `Primitive::External { payload: Rc<dyn Any>, .. }` and hands
/// it back to the registered backend handler on mount.
///
/// Callbacks are reactive: they fire each time the embedded content
/// emits the corresponding event. `url` is reactive too — pass a
/// closure that reads from a `Signal`/`Source` to drive navigation
/// from app state.
pub struct WebViewProps {
    /// Initial + reactive URL. The backend handler subscribes via
    /// `Effect::new(...)`, so changes to the closure's captured
    /// signals re-navigate the WebView. Use [`url`] to coerce any of
    /// `&str` / `String` / `Fn() -> String` into this shape.
    pub url: Box<dyn Fn() -> String>,
    /// Fires for each `postMessage` from the embedded content. The
    /// payload is an opaque string — typically a JSON document the
    /// embedded content `JSON.stringify`'d before posting.
    ///
    /// `Rc` (not `Box`) because the framework hands the SDK a
    /// `Rc<WebViewProps>` (the type-erased `Primitive::External`
    /// payload) — the handler can only borrow, so it clones the `Rc`
    /// into each listener closure rather than moving the inner box.
    pub on_message: Option<Rc<dyn Fn(String)>>,
    /// Fires when the embedded content finishes loading. On web this
    /// is the iframe's `load` event.
    pub on_load: Option<Rc<dyn Fn()>>,
    /// Fires when the embedded content fails to load. On web this is
    /// the iframe's `error` event (which only covers a narrow set of
    /// failure modes — network errors inside the iframe's content
    /// don't bubble up here).
    pub on_error: Option<Rc<dyn Fn()>>,
}

impl Default for WebViewProps {
    fn default() -> Self {
        Self {
            url: Box::new(String::new),
            on_message: None,
            on_load: None,
            on_error: None,
        }
    }
}

/// Coerce any of `&str`, `String`, or `Fn() -> String` into the closure
/// shape [`WebViewProps::url`] stores. Lets the call site write
/// `webview::url("https://...")` for static URLs and
/// `webview::url(move || sig.get())` for reactive ones without thinking
/// about the closure boxing.
pub fn url<U: IntoWebViewUrl>(u: U) -> Box<dyn Fn() -> String> {
    u.into_web_view_url()
}

pub trait IntoWebViewUrl {
    fn into_web_view_url(self) -> Box<dyn Fn() -> String>;
}

impl IntoWebViewUrl for &str {
    fn into_web_view_url(self) -> Box<dyn Fn() -> String> {
        let s = self.to_string();
        Box::new(move || s.clone())
    }
}

impl IntoWebViewUrl for String {
    fn into_web_view_url(self) -> Box<dyn Fn() -> String> {
        Box::new(move || self.clone())
    }
}

impl<F> IntoWebViewUrl for F
where
    F: Fn() -> String + 'static,
{
    fn into_web_view_url(self) -> Box<dyn Fn() -> String> {
        Box::new(self)
    }
}

// ============================================================================
// Handle + ops trait
// ============================================================================

/// Typed handle to a mounted `WebView`. Filled by `Ref::fill` after
/// the primitive mounts; users hold a `Ref<WebViewHandle>` at the call
/// site and reach imperative ops via `r.with(|h| h.reload())`.
///
/// The `ops` pointer is set by the active backend's module via the
/// `OPS` static (see the cfg-gated re-export at the bottom of this
/// file). The `node` is type-erased — each backend's ops downcasts it
/// internally to the concrete native type (`HtmlIFrameElement` /
/// `GlobalRef` / `WKWebView`).
#[derive(Clone)]
pub struct WebViewHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn WebViewOps,
}

impl WebViewHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn WebViewOps) -> Self {
        Self { node, ops }
    }

    /// Send a message to the embedded content. Routes to
    /// `iframe.contentWindow.postMessage(msg, "*")` on web; through
    /// the native message channel on iOS/Android. Payload is opaque —
    /// callers typically JSON-stringify a structured object here and
    /// parse on the other side.
    pub fn post_message(&self, msg: &str) {
        self.ops.post_message(&*self.node, msg);
    }

    /// Reload the embedded content from its current URL.
    pub fn reload(&self) {
        self.ops.reload(&*self.node);
    }

    /// Synchronously evaluate JS inside the embedded content's global
    /// scope. Returns the result as a JSON string (i.e.
    /// `JSON.stringify(value)`). `Err` when the embedded content is
    /// cross-origin, the call throws, or the backend can't do sync
    /// JS eval.
    pub fn execute_js(&self, code: &str) -> Result<String, String> {
        self.ops.execute_js(&*self.node, code)
    }
}

/// Imperative-ops dispatch. Implementations live in each cfg-gated
/// backend module and downcast `node` to their concrete native type.
/// Defaults all no-op so a backend that hasn't wired a particular op
/// degrades silently rather than panicking.
///
/// `Sync` bound: the trait object lives in a `static OPS: &dyn
/// WebViewOps` slot per backend module, which Rust requires to be
/// `Sync`. The ZST impls each backend ships are trivially `Sync`.
pub trait WebViewOps: Sync {
    fn post_message(&self, _node: &dyn Any, _msg: &str) {}
    fn reload(&self, _node: &dyn Any) {}
    fn execute_js(&self, _node: &dyn Any, _code: &str) -> Result<String, String> {
        Err("execute_js not supported by this backend".into())
    }
}

/// Fallback ops used on targets with no `WebView` impl. Every method
/// is a no-op or returns an error; user code keeps compiling but the
/// framework's `External` placeholder is what actually renders.
pub struct UnsupportedOps;
impl WebViewOps for UnsupportedOps {}

// ============================================================================
// Constructor + bind
// ============================================================================

/// Build a `WebView` primitive. Returns a typed `Bound<WebViewHandle>`
/// so `.bind(...)` is type-checked against `Ref<WebViewHandle>`.
///
/// PascalCase intentionally — matches the visual cadence of first-
/// party primitives (`View`, `Button`, `Image`) inside a `ui!` block.
/// Interpolate as `{ webview::WebView(WebViewProps { .. }) }`.
///
/// Under the hood this is `Primitive::External` with a `WebViewProps`
/// payload — same machinery as any other third-party SDK. The marker
/// type on `Bound<H>` is `WebViewHandle` so the `.bind(...)` from
/// [`WebViewBind`] resolves with type-checked refs.
#[allow(non_snake_case)]
pub fn WebView(props: WebViewProps) -> Bound<WebViewHandle> {
    Bound::new(Primitive::External {
        type_id: TypeId::of::<WebViewProps>(),
        type_name: std::any::type_name::<WebViewProps>(),
        payload: Rc::new(props) as Rc<dyn Any>,
        style: None,
        ref_fill: None,
        accessibility: framework_core::accessibility::AccessibilityProps::default(),
    })
}

/// Adds `.bind(r)` to `Bound<WebViewHandle>` via an extension trait
/// (the orphan rule blocks an inherent `impl Bound<WebViewHandle>`
/// here — `Bound` is foreign). Bring this trait into scope to use the
/// builder-style `.bind(...)` on the value returned by [`WebView`].
///
/// Most users don't import this directly — the `prelude` re-export
/// gives them the trait + the constructor + the props struct in one
/// line.
pub trait WebViewBind {
    /// Bind a `Ref<WebViewHandle>` for imperative access. At mount
    /// time the framework calls the `RefFill::External` closure with
    /// the type-erased native node; we wrap it in a `WebViewHandle`
    /// using the cfg-selected backend's `OPS` static and fill the ref.
    fn bind(self, r: Ref<WebViewHandle>) -> Self;
}

impl WebViewBind for Bound<WebViewHandle> {
    fn bind(mut self, r: Ref<WebViewHandle>) -> Self {
        if let Primitive::External { ref_fill, .. } = self.primitive_mut() {
            *ref_fill = Some(RefFill::External(Box::new(move |node_any| {
                r.fill(WebViewHandle::new(node_any, OPS));
            })));
        }
        self
    }
}

/// One-stop import for typical use: `use webview::prelude::*;` brings
/// in the constructor, props struct, handle type, the `.bind(...)`
/// extension trait, and the `url(...)` coercion helper.
pub mod prelude {
    pub use super::{url, WebView, WebViewBind, WebViewHandle, WebViewProps};
}

// ============================================================================
// Backend selector
// ============================================================================

// Each platform module exposes:
//   - `pub fn register(backend: &mut <ConcreteBackend>)`
//   - `pub static OPS: &dyn WebViewOps`
// Only one is compiled per target via cfg; the umbrella re-exports
// both from whichever module matches. On targets with no backend
// support, fallbacks here keep user code compiling — the framework's
// External placeholder is what renders at runtime.

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::register;
#[cfg(target_arch = "wasm32")]
static OPS: &dyn WebViewOps = web::OPS;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub use android::register;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
static OPS: &dyn WebViewOps = android::OPS;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub use ios::register;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
static OPS: &dyn WebViewOps = ios::OPS;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "android",
    target_os = "ios",
)))]
mod fallback {
    use framework_core::Backend;

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
static OPS: &dyn WebViewOps = &UnsupportedOps;
