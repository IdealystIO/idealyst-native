//! New-core surface — the scene-registry leg (idea-lite migration).
//!
//! The old-core leg is an `Element::External` payload dispatched through
//! the per-backend `ExternalRegistry`; that registry does not exist on
//! the new core. The scene [`Registry`] is the new core's unified
//! primitive==external contract, so this leg registers a payload handler
//! there instead:
//!
//! - **Web (wasm32)** — [`register`] installs a `WebBackend`-concrete
//!   handler reproducing the old web handler's DOM call-for-call: one
//!   stable wrapper `<div>` (inline `display: inline-block; line-height:
//!   0`), reactive markup through a world effect that swaps `innerHTML`
//!   (with the same last-markup dedup — re-setting identical markup
//!   tears down browser animation state), `on_load` after each
//!   assignment, author style via [`attach_style`], ref fill with the
//!   web ops (intrinsic-size introspection shared with the old leg via
//!   `web_util`).
//! - **Everywhere else** — [`register`] installs the frozen
//!   External-placeholder degradation path
//!   ([`ExternalOps::create_external`]), mirroring the old walker's
//!   posture for a backend with no registered handler (children +
//!   author style + ref fill + `release_external` teardown all still
//!   flow, exactly like `walker/external.rs`). The native vector
//!   handlers (iOS/Android usvg walk) are old-core-only until the
//!   new-core native boots grow an external story — the codeblock/table
//!   precedent.
//!
//! Same authored call shape as the old-core leg:
//! `Svg(SvgProps { .. }).with_style(…).bind(r)` then element coercion —
//! a consuming app compiles the same source against either core. App
//! bootstrap passes [`register`] as the boot registration seam (there
//! is no inventory self-registration on the new core; the registry is
//! built explicitly at boot).

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::Ref;
use runtime_scene::{item, Element, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::IntoElement;
use runtime_vocabulary::style_attach::{
    attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
};

// ============================================================================
// Public API surface — same shapes as the old core.
// ============================================================================

/// Author-supplied props for an `Svg` instance. Same field shapes as
/// the old core; carried inside the scene item payload and read back by
/// the registered handler.
///
/// `markup` is reactive: the handler subscribes via a world effect and
/// re-renders whenever signals captured by the closure change.
///
/// (No `IdealystSchema` derive on this leg — the MCP catalog documents
/// the old-core surface, per the codeblock/table precedent.)
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
    /// callback fires there — old-core parity.
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
// Handle + ops trait — same shapes as the old core.
// ============================================================================

/// Typed handle to a mounted `Svg`. Filled at mount time when the
/// author chained [`SvgBind::bind`]; user code receives the handle
/// through `Ref::with`.
#[derive(Clone)]
pub struct SvgHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn SvgOps,
}

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

/// Imperative-ops dispatch. Same trait shape as the old core; the
/// active target's `OPS` static supplies the impl.
pub trait SvgOps: Sync {
    /// The SVG's natural `(width, height)` in pixels once parsed, or
    /// `None` before the first successful render.
    fn intrinsic_size(&self, _node: &dyn Any) -> Option<(f32, f32)> {
        None
    }
}

/// Fallback ops used on targets with no `Svg` impl (every non-web
/// target on the new core — the placeholder posture).
pub struct UnsupportedOps;
impl SvgOps for UnsupportedOps {}

#[cfg(target_arch = "wasm32")]
static OPS: &dyn SvgOps = web_glue::OPS;
#[cfg(not(target_arch = "wasm32"))]
static OPS: &dyn SvgOps = &UnsupportedOps;

// ============================================================================
// Payload + builder — the old `Bound<SvgHandle>` call shape
// (`.with_style(…)` / `.bind(…)` then element coercion).
// ============================================================================

/// Scene payload for the `Svg` item. Single-take slots (the vocabulary
/// `PrimCell` discipline, inlined): the scene hands the handler a
/// shared `&Rc<Self>`, but the style/ref-fill must move at mount.
struct SvgPrim {
    props: Rc<SvgProps>,
    style: RefCell<Option<StyleProp>>,
    ref_fill: RefCell<Option<Box<dyn FnOnce(Rc<dyn Any>)>>>,
}

/// Author-side builder returned by [`Svg`] — mirrors the old-core
/// `Bound<SvgHandle>` call shape.
pub struct SvgBound {
    props: Rc<SvgProps>,
    style: Option<StyleProp>,
    ref_fill: Option<Box<dyn FnOnce(Rc<dyn Any>)>>,
}

/// Build an `Svg` primitive — the new-core stand-in for the old-core
/// `Svg(props)`, same call shape at every author site.
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
    /// `<div>` on web), exactly like `.with_style` on the old-core
    /// `Bound`.
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }
}

/// Adds `.bind(r)` — same trait name/shape as the old core so author
/// imports (`use svg::prelude::*`) compile unchanged.
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

/// Mirror of the old core's `From<Bound<H>> for Element`.
impl From<SvgBound> for Element {
    fn from(b: SvgBound) -> Element {
        b.into_element()
    }
}

/// One-stop import: same names as the old-core prelude.
pub mod prelude {
    pub use super::{markup, Svg, SvgBind, SvgHandle, SvgProps};
}

// ============================================================================
// Handlers + registration seam
// ============================================================================

/// Shared mount tail — the old walker's `external.rs` sequence after
/// node creation: (children are none for svg) → author style →
/// ref fill (type-erased node clone) → scope-tied `release_external`.
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
/// External degradation path (`create_external` with no registered
/// old-core handler renders each backend's "not supported" box; SSR
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
        &runtime_core::accessibility::AccessibilityProps::default(),
    );
    finish_mount(&backend, &node, prim);
    node
}

/// Register the svg payload handler on a scene registry. Pass this as
/// the boot registration seam (the `register` argument of
/// `backend_web::newcore::start_in` / `backend_ssr::newcore::
/// render_path_with`).
#[cfg(not(target_arch = "wasm32"))]
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    registry.register::<SvgPrim, _>(mount_placeholder::<H>);
}

/// Register the svg payload handler on the web backend's scene
/// registry — the real `innerHTML` renderer.
#[cfg(target_arch = "wasm32")]
pub fn register(registry: &mut Registry<backend_web::WebBackend>) {
    registry.register::<SvgPrim, _>(web_glue::mount_svg_web);
}

// ============================================================================
// Web glue (wasm32): the old web handler, call-for-call, over the
// scene contract.
// ============================================================================

#[cfg(target_arch = "wasm32")]
mod web_glue {
    use super::*;
    use backend_web::WebBackend;

    pub(super) static OPS: &dyn SvgOps = &WebSvgOps;

    struct WebSvgOps;
    impl SvgOps for WebSvgOps {
        fn intrinsic_size(&self, node: &dyn Any) -> Option<(f32, f32)> {
            // Shared with the old-core leg (web_util) so the two cores'
            // introspection can't drift.
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
        // for reactive markup updates (old-core web handler, verbatim).
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
        // re-fire it — the old `effect!` contract. `on_load` fires
        // inside the flush, so author writes there stage into the same
        // logical update (no manual schedule_flush needed).
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
            // `on_error` doesn't fire on web — old-core parity (see the
            // old web.rs rationale: browsers recover from malformed
            // SVG silently).
            if let Some(cb) = &props_for_effect.on_load {
                cb();
            }
        });

        let node: web_sys::Node = wrapper.into();
        finish_mount(&backend, &node, prim);
        node
    }
}
