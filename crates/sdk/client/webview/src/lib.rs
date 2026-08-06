//! Third-party `WebView` SDK for the idealyst framework.
//!
//! Provides a `WebView` primitive backed by the scene registry — the
//! runtime's unified primitive==external contract. Author-facing API
//! mirrors the framework's other reactive primitives — typed props,
//! `.bind(...)`-able handle, `.with_style(...)`.
//!
//! # Implementation
//!
//! - **Web (wasm32)** — [`register`] installs a `WebBackend`-concrete
//!   handler: one `<iframe>` per mount (inline `border: 0` only —
//!   size/positioning stays with the author's stylesheet), reactive
//!   `src` through a world effect, message/load/error DOM listeners
//!   persisted via the iframe's `__wv_state` reflect slot (see
//!   [`web_util`]), author style via `attach_style`, ref fill with the
//!   web ops (`post_message`/`reload`/`execute_js` bodies also live in
//!   `web_util`).
//! - **Everywhere else** — [`register`] installs the External-placeholder
//!   degradation path ([`ExternalOps::create_external`]): the host's
//!   "not supported" box (a bare `<div>` on SSR), with author style +
//!   ref fill + `release_external` teardown still flowing. Native
//!   iOS/Android `WKWebView`/`android.webkit.WebView` renderers are NOT
//!   implemented — see the crate README/`docs/migrating-to-runtime-v2.md`
//!   for the seam a native leg would fill.
//!
//! # Usage
//!
//! ```ignore
//! // In the app's bootstrap (one line per third-party SDK) — the boot
//! // entry's `register` argument IS the registration seam; there is no
//! // inventory self-registration.
//! backend_web::newcore::start_in("#app", webview::register, app);
//!
//! // Inside a `ui!` block — either the PascalCase tag:
//! ui! {
//!     view {
//!         WebView(url = webview::url("https://example.com"))
//!     }
//! }
//! // …or the fn-call form when a handle is needed:
//! let url = signal("https://example.com".to_string());
//! let wv: Ref<WebViewHandle> = Ref::new();
//! ui! {
//!     view {
//!         { webview::WebView(WebViewProps {
//!             url: webview::url(move || url.get()),
//!             on_load: Some(Rc::new(|| log::info!("loaded"))),
//!             ..Default::default()
//!         }).bind(wv.clone()) }
//!     }
//! }
//! // Imperative ops at any later point:
//! wv.with(|h| h.reload());
//! ```
//!
//! An UNREGISTERED payload panics at realize (the scene contract), so a
//! missed `register` fails loud.
#![deny(missing_docs)]

// Shared wasm32 helpers (pure DOM: imperative iframe ops + the event
// listener wiring with its `__wv_state` closure persistence — no core
// types).
#[cfg(target_arch = "wasm32")]
pub(crate) mod web_util;
// Real WebKitGTK leaf, behind the OFF-by-default `linux-webkit` feature:
// webkit6-sys links the system `webkitgtk-6.0` through pkg-config, so an
// unconditional dep would break the build of this crate — and everything
// depending on it — on any Linux host without that dev package.
#[cfg(all(target_os = "linux", feature = "linux-webkit", not(target_arch = "wasm32")))]
mod linux;

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_scene::{item, Element, Host, MountCx, Registry};
use runtime_shared::Ref;
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::{BuildElement, IntoElement};
use runtime_vocabulary::style_attach::{
    attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
};

// ============================================================================
// Public API surface
// ============================================================================

/// Author-supplied props for a `WebView` instance. Carried inside the
/// scene item payload and read back by the registered handler.
///
/// Callbacks are reactive: they fire each time the embedded content
/// emits the corresponding event. `url` is reactive too — pass a
/// closure that reads from a `Signal`/`Source` to drive navigation
/// from app state.
pub struct WebViewProps {
    /// Initial + reactive URL. The handler subscribes via a world
    /// effect, so changes to the closure's captured signals re-navigate
    /// the WebView. Use [`url`] to coerce any of `&str` / `String` /
    /// `Fn() -> String` into this shape.
    pub url: Box<dyn Fn() -> String>,
    /// Fires for each `postMessage` from the embedded content. The
    /// payload is an opaque string — typically a JSON document the
    /// embedded content `JSON.stringify`'d before posting.
    ///
    /// `Rc` (not `Box`) because the handler owns the props via
    /// `Rc<WebViewProps>` — it can only borrow, so it clones the `Rc`
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
/// shape [`WebViewProps::url`] stores.
pub fn url<U: IntoWebViewUrl>(u: U) -> Box<dyn Fn() -> String> {
    u.into_web_view_url()
}

/// Coercion target for [`url`]. Implemented for `&str`, `String`, and
/// any `Fn() -> String`, so the call site can pass a static or reactive
/// URL interchangeably.
pub trait IntoWebViewUrl {
    /// Box the receiver into the `Fn() -> String` closure that
    /// [`WebViewProps::url`] stores.
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

/// Typed handle to a mounted `WebView`. Filled at mount time when the
/// author chained [`WebViewBind::bind`]; user code receives the handle
/// through `Ref::with` and reaches imperative ops via
/// `r.with(|h| h.reload())`.
#[derive(Clone)]
pub struct WebViewHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn WebViewOps,
}

/// Pointer identity on the NODE — a `WebViewHandle` names one mounted `WebView`, so
/// clones of it are equal and handles onto two different `WebView`s never are.
/// Exactly the shape (and reasoning) of `form::FormHandle`'s impl.
///
/// `node` is a type-erased native element behind `Rc<dyn Any>`: the address
/// is all there is to compare, and it is the right thing to compare. `ops`
/// is excluded deliberately — it is the backend's single `&'static` vtable,
/// identical for every handle on a target, so it says nothing about WHICH
/// `WebView` this is.
///
/// Needed because `Signal<T>` is bounded on `T: PartialEq` at creation and
/// `get`, not just on the guarded `set`; an author stashing the bound handle
/// in state cannot add the impl themselves (orphan rule).
impl PartialEq for WebViewHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.node, &other.node)
    }
}

impl Eq for WebViewHandle {}

impl WebViewHandle {
    /// Wrap a type-erased native node + backend ops into a handle.
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn WebViewOps) -> Self {
        Self { node, ops }
    }

    /// Send a message to the embedded content. Routes to
    /// `iframe.contentWindow.postMessage(msg, "*")` on web. Payload is
    /// opaque — callers typically JSON-stringify a structured object
    /// here and parse on the other side.
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

/// Imperative-ops dispatch. The active target's `OPS` static supplies
/// the impl.
pub trait WebViewOps: Sync {
    /// Post a message into the embedded content. Default no-op.
    fn post_message(&self, _node: &dyn Any, _msg: &str) {}
    /// Reload the embedded content from its current URL. Default no-op.
    fn reload(&self, _node: &dyn Any) {}
    /// Synchronously evaluate JS in the embedded content and return its
    /// JSON-stringified result. Default returns an unsupported error.
    fn execute_js(&self, _node: &dyn Any, _code: &str) -> Result<String, String> {
        Err("execute_js not supported by this backend".into())
    }
}

/// Fallback ops used on targets with no `WebView` impl (every non-web
/// target — the placeholder posture).
pub struct UnsupportedOps;
impl WebViewOps for UnsupportedOps {}

#[cfg(target_arch = "wasm32")]
static OPS: &dyn WebViewOps = web_glue::OPS;
#[cfg(not(target_arch = "wasm32"))]
static OPS: &dyn WebViewOps = &UnsupportedOps;

// ============================================================================
// Payload + builder — the `.with_style(…)` / `.bind(…)` chain then
// element coercion.
// ============================================================================

/// Scene payload for the `WebView` item. Single-take slots (the
/// vocabulary `PrimCell` discipline, inlined): the scene hands the
/// handler a shared `&Rc<Self>`, but the style/ref-fill must move at
/// mount.
struct WebViewPrim {
    props: Rc<WebViewProps>,
    style: RefCell<Option<StyleProp>>,
    ref_fill: RefCell<Option<Box<dyn FnOnce(Rc<dyn Any>)>>>,
}

/// Author-side builder returned by [`WebView`].
pub struct WebViewBound {
    props: Rc<WebViewProps>,
    style: Option<StyleProp>,
    ref_fill: Option<Box<dyn FnOnce(Rc<dyn Any>)>>,
}

/// Build a `WebView` primitive.
#[allow(non_snake_case)]
pub fn WebView(props: WebViewProps) -> WebViewBound {
    WebViewBound {
        props: Rc::new(props),
        style: None,
        ref_fill: None,
    }
}

impl WebViewBound {
    /// Attach the author style — lands on the mounted node (the
    /// `<iframe>` on web), exactly like `.with_style` on a framework
    /// primitive's builder.
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }
}

/// Adds `.bind(r)` so `use webview::prelude::*` brings the imperative
/// binding into scope.
pub trait WebViewBind {
    /// Bind a `Ref<WebViewHandle>` for imperative access. At mount
    /// time the handler wraps the native node in a `WebViewHandle`
    /// using the active target's ops and fills the ref.
    fn bind(self, r: Ref<WebViewHandle>) -> Self;
}

impl WebViewBind for WebViewBound {
    fn bind(mut self, r: Ref<WebViewHandle>) -> Self {
        self.ref_fill = Some(Box::new(move |node_any| {
            r.fill(WebViewHandle::new(node_any, OPS));
        }));
        self
    }
}

impl IntoElement for WebViewBound {
    fn into_element(self) -> Element {
        item(
            WebViewPrim {
                props: self.props,
                style: RefCell::new(self.style),
                ref_fill: RefCell::new(self.ref_fill),
            },
            Vec::new(),
        )
    }
}

/// Element coercion for the fn-call form.
impl From<WebViewBound> for Element {
    fn from(b: WebViewBound) -> Element {
        b.into_element()
    }
}

// ============================================================================
// `ui!` dispatch — type alias + BuildElement impl (glue dispatch).
//
// The `ui!` macro lowers a PascalCase tag `WebView(url = …)` to a
// struct literal `BuildElement::build(WebView { url: …, .. })`, so the
// tag name must resolve as a *type* with a `BuildElement` impl. The fn
// [`WebView`] (value namespace) and this alias (type namespace)
// coexist — the Card precedent.
// ============================================================================

/// `ui!` tag alias for the WebView — `ui! { WebView(url = …) }`
/// resolves to this type and dispatches through [`BuildElement`].
///
/// The tag form yields an `Element` (the handle is dropped) — to bind
/// a `Ref<WebViewHandle>` for imperative ops, use the fn-call form and
/// its `.bind(..)` chain: `WebView(props).bind(r)`.
pub type WebView = WebViewProps;

impl BuildElement for WebViewProps {
    fn build(self) -> Element {
        WebView(self).into_element()
    }
}

/// One-stop import for the author-facing names.
pub mod prelude {
    pub use super::{url, WebView, WebViewBind, WebViewHandle, WebViewProps};
}

// ============================================================================
// Handlers + registration seam
// ============================================================================

/// Shared mount tail: (children are none for webview) → author style →
/// ref fill (type-erased node clone) → scope-tied `release_external`.
fn finish_mount<H>(backend: &Rc<RefCell<H>>, node: &H::Node, prim: &WebViewPrim)
where
    H: ExternalOps + StyleServices,
{
    if let Some(style) = prim.style.borrow_mut().take() {
        attach_style(backend, node, style);
    }
    if let Some(fill) = prim.ref_fill.borrow_mut().take() {
        let any_node: Rc<dyn Any> = Rc::new(node.clone());
        fill(any_node);
    }
    let backend = backend.clone();
    let node = node.clone();
    on_teardown(move || {
        backend.borrow_mut().release_external(&node);
    });
}

/// Placeholder handler for hosts with no real webview renderer — the
/// External degradation path (`create_external` renders each backend's
/// "not supported" box; SSR renders a bare `<div>`).
#[cfg(not(target_arch = "wasm32"))]
fn mount_placeholder<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<WebViewPrim>,
    _children: Vec<Element>,
) -> H::Node
where
    H: ExternalOps + StyleServices,
{
    let backend = cx.backend().clone();
    let payload: Rc<dyn Any> = prim.props.clone();
    let node = backend.borrow_mut().create_external(
        std::any::TypeId::of::<WebViewProps>(),
        std::any::type_name::<WebViewProps>(),
        &payload,
        &runtime_shared::accessibility::AccessibilityProps::default(),
    );
    finish_mount(&backend, &node, prim);
    node
}

/// Linux (GTK4) mount handler — `Registry<LinuxBackend>`-concrete. A real
/// embedded browser via WebKitGTK 6 (`webkit6::WebView`).
#[cfg(all(target_os = "linux", feature = "linux-webkit", not(target_arch = "wasm32")))]
fn mount_webview_linux(
    cx: &mut MountCx<'_, backend_linux::LinuxBackend>,
    prim: &Rc<WebViewPrim>,
    _children: Vec<Element>,
) -> backend_linux::LinuxNode {
    let backend = cx.backend().clone();
    let node = crate::linux::build_web_view(&prim.props, &mut backend.borrow_mut());
    finish_mount(&backend, &node, prim);
    node
}

/// Register the webview payload handler on a scene registry. Pass this
/// as the boot registration seam (the `register` argument of
/// `backend_web::newcore::start_in` / `backend_ssr::newcore::
/// render_path_with`).
#[cfg(not(target_arch = "wasm32"))]
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    #[cfg(all(target_os = "linux", feature = "linux-webkit", not(target_arch = "wasm32")))]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_linux::LinuxBackend>>() {
            reg.register::<WebViewPrim, _>(mount_webview_linux);
            return;
        }
    }
    // Feature off (or no WebKitGTK): fall through to the placeholder — the
    // same graceful degradation an unsupported target gets.
    registry.register::<WebViewPrim, _>(mount_placeholder::<H>);
}

/// Register the webview payload handler on the web backend's scene
/// registry — the real `<iframe>` renderer.
#[cfg(target_arch = "wasm32")]
pub fn register(registry: &mut Registry<backend_web::WebBackend>) {
    registry.register::<WebViewPrim, _>(web_glue::mount_webview_web);
}

/// Declare this SDK's payload kind **late-bound** instead of installing
/// its handler — the boot half of lazy registration. Pair with
/// [`register_from_chunk`] from inside a `#[component(lazy)]` body.
///
/// Only web code-splits, so on every other target this installs the
/// handler eagerly exactly as [`register`] does. That is deliberate:
/// deferring a kind nothing later registers leaves the payload parked
/// behind a placeholder forever, with no panic and no log, and native
/// has no chunk to arrive. Calling `defer` is therefore always safe —
/// it splits where splitting exists and is a plain `register` elsewhere.
pub fn defer<H>(registry: &mut Registry<H>)
where
    H: Host + ExternalOps + StyleServices + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        registry.defer::<WebViewPrim>();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        register(registry);
    }
}

/// Install the web payload handler from inside a lazy chunk — the chunk
/// half of lazy registration. Requires [`defer`] at boot.
///
/// Web-only by construction: the web handler is `WebBackend`-concrete,
/// and web is the only target that code-splits. The non-web build is an
/// empty stub so a `#[component(lazy)]` body calling this compiles on
/// every target — there, [`defer`] already registered eagerly.
#[cfg(target_arch = "wasm32")]
pub fn register_from_chunk() {
    runtime_scene::defer_registration::<backend_web::WebBackend, _>(|registry| {
        registry.register_deferred::<WebViewPrim, _>(web_glue::mount_webview_web);
    });
}

/// Non-web stub — see the wasm32 [`register_from_chunk`].
#[cfg(not(target_arch = "wasm32"))]
pub fn register_from_chunk() {}

// ============================================================================
// Web glue (wasm32): the real `<iframe>` renderer over the scene
// contract.
// ============================================================================

#[cfg(target_arch = "wasm32")]
mod web_glue {
    use super::*;
    use backend_web::WebBackend;

    pub(super) static OPS: &dyn WebViewOps = &WebWebViewOps;

    struct WebWebViewOps;
    impl WebViewOps for WebWebViewOps {
        // Bodies live in `web_util` alongside the listener wiring.
        fn post_message(&self, node: &dyn Any, msg: &str) {
            crate::web_util::post_message(node, msg);
        }
        fn reload(&self, node: &dyn Any) {
            crate::web_util::reload(node);
        }
        fn execute_js(&self, node: &dyn Any, code: &str) -> Result<String, String> {
            crate::web_util::execute_js(node, code)
        }
    }

    /// Wrap an author callback so a trailing
    /// [`backend_web::newcore::schedule_flush`] commits whatever it
    /// staged.
    ///
    /// "External glue must call schedule_flush": the DOM listeners
    /// wired below fire author callbacks OUTSIDE the runtime's wrapped
    /// dispatch sites (the residual surface documented in
    /// backend-web/src/newcore.rs's module docs — closed for webview
    /// here). Without this, a signal write inside `on_message` /
    /// `on_load` / `on_error` would stay staged forever. The reactive
    /// URL effect below does NOT need it — effects run inside the
    /// flush.
    fn flush_after(cb: Rc<dyn Fn()>) -> Rc<dyn Fn()> {
        Rc::new(move || {
            cb();
            backend_web::newcore::schedule_flush();
        })
    }

    /// [`flush_after`] for the string-payload `on_message` shape.
    fn flush_after_msg(cb: Rc<dyn Fn(String)>) -> Rc<dyn Fn(String)> {
        Rc::new(move |payload| {
            cb(payload);
            backend_web::newcore::schedule_flush();
        })
    }

    pub(super) fn mount_webview_web(
        cx: &mut MountCx<'_, WebBackend>,
        prim: &Rc<WebViewPrim>,
        _children: Vec<Element>,
    ) -> web_sys::Node {
        let backend = cx.backend().clone();
        let document = web_sys::window()
            .expect("no window")
            .document()
            .expect("no document");
        let iframe = document
            .create_element("iframe")
            .expect("create_element(iframe) failed");
        // Only `border: 0` set inline — keeping size/positioning to the
        // author's stylesheet (inline `style` would override class
        // rules).
        let _ = iframe.set_attribute("style", "border: 0");
        let _ = iframe.set_attribute("data-external-kind", "webview::WebViewProps");

        // Reactive src. World effect: created during realize (world
        // entered), so it is collected into the enclosing subtree and
        // dies at unmount; it runs once immediately and re-fires when
        // signals read by the author's url closure change — the
        // `effect!` contract.
        let iframe_for_url = iframe.clone();
        let props_for_effect = prim.props.clone();
        runtime_world::effect(move || {
            let url = (props_for_effect.url)();
            let _ = iframe_for_url.set_attribute("src", &url);
        });

        // Author-callback listeners + `__wv_state` closure persistence
        // (see `web_util`), each callback wrapped to schedule_flush
        // after it returns.
        crate::web_util::wire_listeners(
            &iframe,
            prim.props.on_message.clone().map(flush_after_msg),
            prim.props.on_load.clone().map(flush_after),
            prim.props.on_error.clone().map(flush_after),
        );

        let node: web_sys::Node = iframe.into();
        finish_mount(&backend, &node, prim);
        node
    }
}
