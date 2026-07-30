//! Third-party SVG renderer SDK for the idealyst framework.
//!
//! Provides an `Svg` primitive rendered from SVG markup. The mechanism
//! differs per platform but the output converges (CLAUDE.md §7):
//!
//! - **Web (wasm32)** — a stable wrapper `<div>` (inline
//!   `display: inline-block; line-height: 0`) whose `innerHTML` is the
//!   markup, so the browser's own spec-compliant vector renderer draws
//!   it. Reactive markup rides a world effect with a last-markup dedup
//!   (re-setting identical `innerHTML` tears down browser animation
//!   state), and `on_load` fires after each assignment.
//! - **iOS / Android** — `usvg` parses to a normalized tree and the
//!   walker in [`tree_walker`](crate) emits *native vector primitives*:
//!   `UIBezierPath` + `CGContext` on iOS, `android.graphics.Path` +
//!   `Canvas` recorded into a `Picture` on Android. No rasterization
//!   step — the view's `drawRect:` / `PictureDrawable.draw(canvas)`
//!   re-runs at the layout-time pixel resolution, so the output stays
//!   crisp at any scale and through transform animations.
//! - **Every other host (SSR, terminal, gpu, host-mock)** — the frozen
//!   External-placeholder degradation path
//!   ([`ExternalOps::create_external`]): children + author style + ref
//!   fill + `release_external` teardown still flow, but nothing is
//!   painted.
//!
//! [`register`] is the boot registration seam and picks the right
//! handler. On non-wasm targets it type-dispatches ONCE at registration
//! (the toolbar SDK's pattern), because a cfg split alone cannot tell an
//! iOS app build from an SSG build that happens to run on the same host
//! OS.
//!
//! # Author callbacks need no explicit flush
//!
//! `on_load` / `on_error` fire from INSIDE the world effect that
//! re-reads the markup — on every platform, web and native alike. That
//! body already runs within a flush, so an author signal write there
//! stages into the same logical update and no backend
//! `newcore::schedule_flush()` call is needed. The rule that DOES apply
//! (see each backend's `newcore.rs` module docs) is for glue that fires
//! author code from a platform event source outside the framework's
//! dispatch sites; this SDK has none.
//!
//! # Scope
//!
//! - Paths + fills (solid, linear gradient, radial gradient)
//! - Paths + strokes (width, linecap, linejoin, miter limit, dash array)
//! - Per-element opacity and fill-rule
//! - Nested groups (transforms compose through `Path::abs_transform`,
//!   which usvg pre-resolves)
//! - Reactive inline SVG markup. URL sources can be wrapped by author
//!   code (fetch bytes via the standard `image` pipeline, pass the
//!   markup string here).
//!
//! Known gaps: filters (`feGaussianBlur`, color matrices, …), masks +
//! clipPaths (paths still draw, unclipped), group opacity != 1 (applied
//! to children's fills/strokes rather than via offscreen compositing),
//! embedded raster `<image>` elements. The walker can be extended for
//! any of these without touching the per-backend painter surface — see
//! `src/tree_walker.rs` for the `SvgPainter` trait that bridges to
//! native primitives.
//!
//! # Usage
//!
//! ```ignore
//! // App bootstrap — `register` IS the seam:
//! backend_web::newcore::start_in("#app", svg::register, app);
//!
//! // Inside a `ui!` block. `Svg` interpolates as an expression — the
//! // macro only knows the closed first-party set, so third-party
//! // primitives come in via `{ ... }` interpolation.
//! let markup = signal(LOGO_SVG.to_string());
//! let r: Ref<SvgHandle> = Ref::new();
//! ui! {
//!     view {
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
#![deny(missing_docs)]

// Shared wasm32 helpers (pure DOM introspection, no framework types).
#[cfg(target_arch = "wasm32")]
pub(crate) mod web_util;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;

// The walker translates a parsed `usvg::Tree` into trait-driven calls
// against per-backend native vector primitives. Only the iOS and
// Android painters consume it.
#[cfg(all(
    any(target_os = "ios", target_os = "android"),
    not(target_arch = "wasm32")
))]
pub(crate) mod tree_walker;

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::Ref;
use runtime_scene::{item, Element, Host, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{
    attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
};

// ============================================================================
// Public API surface
// ============================================================================

/// Author-supplied props for an `Svg` instance. Carried inside the scene
/// item payload and read back by the registered handler.
///
/// `markup` is reactive: the handler subscribes via a world effect and
/// re-renders whenever signals captured by the closure change.
pub struct SvgProps {
    /// Initial + reactive SVG markup. Use [`markup`] to coerce `&str`,
    /// `String`, or `Fn() -> String` into this closure shape.
    pub markup: Box<dyn Fn() -> String>,
    /// Fires after every successful render (once per `innerHTML`
    /// assignment on web). `Rc` (not `Box`) because the handler owns
    /// the props via `Rc<SvgProps>` and clones the callback into effect
    /// bodies.
    pub on_load: Option<Rc<dyn Fn()>>,
    /// Fires when the markup fails to parse. Not observable on web
    /// (browsers accept malformed SVG and render partial trees), so no
    /// callback fires there; on iOS / Android it carries usvg's parse
    /// error.
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
/// shape [`SvgProps::markup`] expects.
pub fn markup<U: IntoSvgMarkup>(u: U) -> Box<dyn Fn() -> String> {
    u.into_svg_markup()
}

/// Coercion target for [`markup`]. Implemented for `&str`, `String`,
/// and any `Fn() -> String`.
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

/// Typed handle to a mounted `Svg`. Filled at mount time when the
/// author chained [`SvgBind::bind`]; user code receives the handle
/// through `Ref::with`.
#[derive(Clone)]
pub struct SvgHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn SvgOps,
}

/// Pointer identity on the NODE — a `SvgHandle` names one mounted `Svg`, so
/// clones of it are equal and handles onto two different `Svg`s never are.
/// Exactly the shape (and reasoning) of `form::FormHandle`'s impl.
///
/// `node` is a type-erased native element behind `Rc<dyn Any>`: the address
/// is all there is to compare, and it is the right thing to compare. `ops`
/// is excluded deliberately — it is the backend's single `&'static` vtable,
/// identical for every handle on a target, so it says nothing about WHICH
/// `Svg` this is.
///
/// Needed because `Signal<T>` is bounded on `T: PartialEq` at creation and
/// `get`, not just on the guarded `set`; an author stashing the bound handle
/// in state cannot add the impl themselves (orphan rule).
impl PartialEq for SvgHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.node, &other.node)
    }
}

impl Eq for SvgHandle {}

impl SvgHandle {
    /// Wrap a type-erased native node + backend ops into a handle.
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn SvgOps) -> Self {
        Self { node, ops }
    }

    /// The SVG's natural pixel dimensions, as declared by its viewBox
    /// (or `width`/`height` attributes if no viewBox is present).
    /// Returns `None` until the first successful render.
    pub fn intrinsic_size(&self) -> Option<(f32, f32)> {
        self.ops.intrinsic_size(&*self.node)
    }
}

/// Imperative-ops dispatch. The active target's `OPS` static supplies
/// the impl, which downcasts `node` to its concrete native type.
pub trait SvgOps: Sync {
    /// The SVG's natural `(width, height)` in pixels once parsed, or
    /// `None` before the first successful render.
    fn intrinsic_size(&self, _node: &dyn Any) -> Option<(f32, f32)> {
        None
    }
}

/// Fallback ops used on targets with no `Svg` renderer (the
/// placeholder posture).
pub struct UnsupportedOps;
impl SvgOps for UnsupportedOps {}

#[cfg(target_arch = "wasm32")]
static OPS: &dyn SvgOps = web_glue::OPS;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
static OPS: &dyn SvgOps = crate::ios::OPS;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
static OPS: &dyn SvgOps = crate::android::OPS;
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
static OPS: &dyn SvgOps = &UnsupportedOps;

// ============================================================================
// Payload + builder — `.with_style(…)` / `.bind(…)` then element
// coercion.
// ============================================================================

/// Scene payload for the `Svg` item. Single-take slots (the vocabulary
/// `PrimCell` discipline, inlined): the scene hands the handler a
/// shared `&Rc<Self>`, but the style/ref-fill must move at mount.
struct SvgPrim {
    props: Rc<SvgProps>,
    style: RefCell<Option<StyleProp>>,
    ref_fill: RefCell<Option<Box<dyn FnOnce(Rc<dyn Any>)>>>,
}

/// Author-side builder returned by [`Svg`].
pub struct SvgBound {
    props: Rc<SvgProps>,
    style: Option<StyleProp>,
    ref_fill: Option<Box<dyn FnOnce(Rc<dyn Any>)>>,
}

/// Build an `Svg` primitive.
///
/// PascalCase intentionally — it matches the visual cadence of the
/// first-party primitives inside a `ui!` block. Interpolate as
/// `{ svg::Svg(SvgProps { .. }) }`.
#[allow(non_snake_case)]
pub fn Svg(props: SvgProps) -> SvgBound {
    SvgBound {
        props: Rc::new(props),
        style: None,
        ref_fill: None,
    }
}

impl SvgBound {
    /// Attach the author style — lands on the outer node (the wrapper
    /// `<div>` on web, the native view elsewhere).
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }
}

/// Adds `.bind(r)`. Bring it into scope (`use svg::prelude::*`) to
/// chain the bind on the value [`Svg`] returns.
pub trait SvgBind {
    /// Bind a `Ref<SvgHandle>` for imperative access. At mount time the
    /// handler wraps the native node in an `SvgHandle` using the active
    /// target's ops and fills the ref.
    fn bind(self, r: Ref<SvgHandle>) -> Self;
}

impl SvgBind for SvgBound {
    fn bind(mut self, r: Ref<SvgHandle>) -> Self {
        self.ref_fill = Some(Box::new(move |node_any| {
            r.fill(SvgHandle::new(node_any, OPS));
        }));
        self
    }
}

impl IntoElement for SvgBound {
    fn into_element(self) -> Element {
        item(
            SvgPrim {
                props: self.props,
                style: RefCell::new(self.style),
                ref_fill: RefCell::new(self.ref_fill),
            },
            Vec::new(),
        )
    }
}

/// Element coercion for bare `{ … }` interpolation sites.
impl From<SvgBound> for Element {
    fn from(b: SvgBound) -> Element {
        b.into_element()
    }
}

/// One-stop import: the constructor, props struct, handle type, the
/// `.bind(...)` extension trait, and the `markup(...)` coercion
/// helper.
pub mod prelude {
    pub use super::{markup, Svg, SvgBind, SvgHandle, SvgProps};
}

// ============================================================================
// Handlers + registration seam
// ============================================================================

/// Shared mount tail after node creation: (svg has no children) →
/// author style → ref fill (type-erased node clone) → scope-tied
/// `release_external` teardown.
fn finish_mount<H>(backend: &Rc<RefCell<H>>, node: &H::Node, prim: &SvgPrim)
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

/// Placeholder handler for hosts with no real svg renderer — the frozen
/// External degradation path (each backend's "not supported" box; SSR
/// renders a bare `<div>`).
#[cfg(not(target_arch = "wasm32"))]
fn mount_placeholder<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<SvgPrim>,
    _children: Vec<Element>,
) -> H::Node
where
    H: ExternalOps + StyleServices,
{
    let backend = cx.backend().clone();
    let payload: Rc<dyn Any> = prim.props.clone();
    let node = backend.borrow_mut().create_external(
        std::any::TypeId::of::<SvgProps>(),
        std::any::type_name::<SvgProps>(),
        &payload,
        &runtime_shared::accessibility::AccessibilityProps::default(),
    );
    finish_mount(&backend, &node, prim);
    node
}

/// iOS mount handler — `Registry<IosBackend>`-concrete (the UIKit view
/// subclass + `usvg` walk have no caps-trait expression). Wraps the
/// native builder in `ios.rs` and runs the standard mount tail.
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
fn mount_svg_ios(
    cx: &mut MountCx<'_, backend_ios::IosBackend>,
    prim: &Rc<SvgPrim>,
    _children: Vec<Element>,
) -> backend_ios::IosNode {
    let backend = cx.backend().clone();
    let node = crate::ios::build_svg(&prim.props, &mut backend.borrow_mut());
    finish_mount(&backend, &node, prim);
    node
}

/// Android mount handler — `Registry<AndroidBackend>`-concrete.
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
fn mount_svg_android(
    cx: &mut MountCx<'_, backend_android::AndroidBackend>,
    prim: &Rc<SvgPrim>,
    _children: Vec<Element>,
) -> jni::objects::GlobalRef {
    let backend = cx.backend().clone();
    let node = crate::android::build_svg(&prim.props, &mut backend.borrow_mut());
    finish_mount(&backend, &node, prim);
    node
}

/// Register the svg payload handler on a scene registry. Pass this as
/// the boot registration seam (the `register` argument of
/// `backend_web::newcore::start_in` / `backend_ssr::newcore::
/// render_path_with` / a native host's `run_with`).
///
/// The platform dispatch happens ONCE here, by registry type: on iOS /
/// Android the concrete native registry gets the real vector-walk
/// handler, and every other host gets the External placeholder. A cfg
/// split alone could not express that — `target_os = "ios"` is also
/// true for a host-side SSR render inside an iOS app build graph.
#[cfg(not(target_arch = "wasm32"))]
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    #[cfg(target_os = "ios")]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_ios::IosBackend>>() {
            reg.register::<SvgPrim, _>(mount_svg_ios);
            return;
        }
    }
    #[cfg(target_os = "android")]
    {
        let any: &mut dyn Any = registry;
        if let Some(reg) = any.downcast_mut::<Registry<backend_android::AndroidBackend>>() {
            reg.register::<SvgPrim, _>(mount_svg_android);
            return;
        }
    }
    registry.register::<SvgPrim, _>(mount_placeholder::<H>);
}

/// Register the svg payload handler on the web backend's scene
/// registry — the real `innerHTML` renderer.
#[cfg(target_arch = "wasm32")]
pub fn register(registry: &mut Registry<backend_web::WebBackend>) {
    registry.register::<SvgPrim, _>(web_glue::mount_svg_web);
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
///
/// [`SvgPrim`] is private, so `registry.defer::<…>()` is not something a
/// consumer could write themselves.
pub fn defer<H>(registry: &mut Registry<H>)
where
    H: Host + ExternalOps + StyleServices + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        registry.defer::<SvgPrim>();
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
        registry.register_deferred::<SvgPrim, _>(web_glue::mount_svg_web);
    });
}

/// Non-web stub — see the wasm32 [`register_from_chunk`].
#[cfg(not(target_arch = "wasm32"))]
pub fn register_from_chunk() {}

// ============================================================================
// Web glue (wasm32).
// ============================================================================

#[cfg(target_arch = "wasm32")]
mod web_glue {
    use super::*;
    use backend_web::WebBackend;

    pub(super) static OPS: &dyn SvgOps = &WebSvgOps;

    struct WebSvgOps;
    impl SvgOps for WebSvgOps {
        fn intrinsic_size(&self, node: &dyn Any) -> Option<(f32, f32)> {
            crate::web_util::intrinsic_size_of_node(node)
        }
    }

    pub(super) fn mount_svg_web(
        cx: &mut MountCx<'_, WebBackend>,
        prim: &Rc<SvgPrim>,
        _children: Vec<Element>,
    ) -> web_sys::Node {
        let backend = cx.backend().clone();
        let document = web_sys::window()
            .expect("no window")
            .document()
            .expect("no document");
        // A bare wrapper — author-provided style classes drive size. The
        // wrapper's purpose is to give the framework a stable element to
        // attach frames + classes to while we swap `innerHTML` freely
        // for reactive markup updates.
        let wrapper = document
            .create_element("div")
            .expect("create_element(div) failed");
        let _ = wrapper.set_attribute("data-external-kind", "svg::SvgProps");
        let _ = wrapper.set_attribute("style", "display: inline-block; line-height: 0");

        // Cache the last applied markup so the effect skips no-op
        // re-runs — re-setting identical `innerHTML` tears down + and
        // rebuilds DOM children, resetting browser animation state.
        let last = Rc::new(RefCell::new(String::new()));

        let wrapper_for_effect = wrapper.clone();
        let props_for_effect = prim.props.clone();
        let last_for_effect = last.clone();
        // World effect: created during realize (world entered), so it is
        // collected into the enclosing subtree and dies at unmount. The
        // body's signal reads (inside the author's markup closure)
        // re-fire it. `on_load` fires inside the flush, so author
        // writes there stage into the same logical update (no manual
        // schedule_flush needed).
        runtime_world::effect(move || {
            let markup = (props_for_effect.markup)();
            {
                let cached = last_for_effect.borrow();
                if *cached == markup {
                    return;
                }
            }
            wrapper_for_effect.set_inner_html(&markup);
            *last_for_effect.borrow_mut() = markup;
            // `on_error` doesn't fire on web: browsers recover from
            // malformed SVG silently, so there is nothing to observe.
            if let Some(cb) = &props_for_effect.on_load {
                cb();
            }
        });

        let node: web_sys::Node = wrapper.into();
        finish_mount(&backend, &node, prim);
        node
    }
}
