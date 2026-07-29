//! New-core surface — the scene-registry leg (idea-lite migration).
//!
//! The old-core leg is an `Element::External` payload (WITH children)
//! dispatched through the per-backend `ExternalRegistry`; that registry
//! does not exist on the new core. The scene [`Registry`] is the new
//! core's unified primitive==external contract, so this leg registers a
//! payload handler there instead:
//!
//! - **Web (wasm32)** — [`register`] installs a `WebBackend`-concrete
//!   handler reproducing the old web handler's DOM call-for-call: one
//!   real `<form>` element, the native `submit` event wired to
//!   `preventDefault()` + the author `on_submit` (plus the mandatory
//!   post-callback `schedule_flush` — see the handler), the
//!   `__form_state` reflect-slot keeping the listener closure alive,
//!   children realized as real DOM descendants of the `<form>` (that's
//!   what makes browser autofill + submit-on-enter work), author style,
//!   ref fill with the web ops (`requestSubmit`, shared with the old
//!   leg via `web_util`).
//! - **Everywhere else** — [`register`] installs the
//!   External-placeholder path ([`ExternalOps::create_external`]) with
//!   the children realized INTO the returned node, mirroring the old
//!   walker's `external.rs` order (create → children → style → ref fill
//!   → cleanup). Behaviorally this is the same passthrough-container
//!   posture the old iOS/Android handlers provided (children lay out
//!   inside a plain container; submission is the author's button
//!   calling `on_submit`) — those native modules (UIView / FrameLayout)
//!   stay old-core-only until the new-core native boots grow an
//!   external story. A future native port must also route author
//!   callbacks through the platform's post-dispatch flush seam (the
//!   `schedule_flush` residual noted in `backend-web/src/newcore.rs`'s
//!   module docs applies to every external glue that runs author code
//!   from a raw platform event).
//!
//! Same authored surface as the old-core leg: `FormProps` /
//! `form(props)` / the PascalCase `Form` `ui!` tag (manual
//! `BuildElement` impl — the table precedent; no `#[component]` macro
//! on this leg) / `FormHandle` + `FormBuilder::bind` — a consuming app
//! compiles the same source against either core. App bootstrap passes
//! [`register`] as the boot registration seam (there is no inventory
//! self-registration on the new core; the registry is built explicitly
//! at boot).

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::Ref;
use runtime_scene::{item, Element, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::{BuildElement, IntoElement};
use runtime_vocabulary::style_attach::{
    attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
};

// ============================================================================
// Public API surface — same shapes as the old core.
// ============================================================================

/// Author-supplied props for a `Form`. Same field shapes as the old
/// core; `on_submit` rides the scene item payload and is read back by
/// the registered handler.
///
/// (No `IdealystSchema` derive on this leg — the MCP catalog documents
/// the old-core surface, per the svg/codeblock/table precedent.)
#[derive(Default)]
pub struct FormProps {
    /// The submit action. On web it fires on the native `<form>` submit
    /// event (Enter in a field or a `type="submit"` descendant) AFTER
    /// `preventDefault()`. On native it is invoked by the author's
    /// submit button. Read your field signals inside this closure.
    ///
    /// `Rc` (not `Box`) because the handler owns the payload via
    /// `Rc<FormPrim>` and clones the callback into the event listener.
    /// Share the same `Rc` with your submit button so one closure
    /// covers every backend.
    pub on_submit: Option<Rc<dyn Fn()>>,
    /// Form contents. They ride the scene item's children slot and the
    /// handler realizes them INTO the node it returns: on web they
    /// become real DOM descendants of the `<form>` (required for
    /// autofill + submit-on-enter); elsewhere they're laid out inside
    /// the passthrough container. Populated for you by the `ui!`/`jsx!`
    /// children block.
    pub children: Vec<Element>,
}

// ============================================================================
// Handle + ops trait — same shapes as the old core.
// ============================================================================

/// Typed handle to a mounted `Form`. Filled at mount time when the
/// author chained [`FormBuilder::bind`]; user code receives the handle
/// through `Ref::with`.
#[derive(Clone)]
pub struct FormHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn FormOps,
}

impl FormHandle {
    /// Wrap a type-erased native form node + its backend ops vtable.
    /// Called by the mount-time ref fill; you don't construct this
    /// directly.
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn FormOps) -> Self {
        Self { node, ops }
    }

    /// Programmatically submit the form. On web this calls
    /// `form.requestSubmit()`, which runs constraint validation and
    /// fires the same `submit` event the SDK wired to `on_submit`. On
    /// native there is no form submit event, so this is a no-op —
    /// trigger submission by invoking your `on_submit` closure directly.
    pub fn submit(&self) {
        self.ops.submit(&*self.node);
    }
}

/// Imperative-ops dispatch. Same trait shape as the old core; the
/// active target's `OPS` static supplies the impl.
pub trait FormOps: Sync {
    /// Submit the form represented by `node`. The web impl downcasts to
    /// the concrete `<form>` element and triggers submission; other
    /// targets keep the default no-op since there's no form machinery.
    fn submit(&self, _node: &dyn Any) {}
}

/// Fallback ops used on targets with no `Form` impl (every non-web
/// target on the new core — the placeholder posture).
pub struct UnsupportedOps;
impl FormOps for UnsupportedOps {}

#[cfg(target_arch = "wasm32")]
static OPS: &dyn FormOps = web_glue::OPS;
#[cfg(not(target_arch = "wasm32"))]
static OPS: &dyn FormOps = &UnsupportedOps;

// ============================================================================
// Payload + builder — the old `Bound<FormHandle>` call shape
// (`.with_style(…)` / `.bind(…)` then element coercion).
// ============================================================================

/// Scene payload for the `Form` item. Single-take slots (the vocabulary
/// `PrimCell` discipline, inlined): the scene hands the handler a
/// shared `&Rc<Self>`, but the style/ref-fill must move at mount.
/// Children do NOT ride the payload — they ride the scene item's
/// children slot and the handler parents them (the old walker's
/// `insert_children` contract).
struct FormPrim {
    on_submit: Option<Rc<dyn Fn()>>,
    style: RefCell<Option<StyleProp>>,
    ref_fill: RefCell<Option<Box<dyn FnOnce(Rc<dyn Any>)>>>,
}

/// Author-side builder returned by [`form`] — mirrors the old-core
/// `Bound<FormHandle>` call shape.
pub struct FormBound {
    on_submit: Option<Rc<dyn Fn()>>,
    children: Vec<Element>,
    style: Option<StyleProp>,
    ref_fill: Option<Box<dyn FnOnce(Rc<dyn Any>)>>,
}

/// Build a `Form` container programmatically — the new-core stand-in
/// for the old-core `form(props)`, same call shape at every author
/// site. The PascalCase [`Form`] tag delegates here; use this fn-call
/// form (`form(props).bind(r)`) when you need the handle.
pub fn form(mut props: FormProps) -> FormBound {
    // Children ride the scene item's children slot (the handler parents
    // them); the payload only needs to carry `on_submit` — same move
    // the old core made before type-erasing the props.
    let children = std::mem::take(&mut props.children);
    FormBound {
        on_submit: props.on_submit,
        children,
        style: None,
        ref_fill: None,
    }
}

impl FormBound {
    /// Attach the author style — lands on the `<form>` node on web, on
    /// the placeholder container elsewhere, exactly like `.with_style`
    /// on the old-core `Bound`.
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }
}

/// Adds `.bind(r)` — same trait name/shape as the old core so author
/// imports (`use form::prelude::*`) compile unchanged.
pub trait FormBuilder {
    /// Bind a `Ref<FormHandle>` for imperative access (e.g.
    /// `r.with(|h| h.submit())`). At mount time the handler wraps the
    /// native node in a `FormHandle` using the active target's ops and
    /// fills the ref.
    fn bind(self, r: Ref<FormHandle>) -> Self;
}

impl FormBuilder for FormBound {
    fn bind(mut self, r: Ref<FormHandle>) -> Self {
        self.ref_fill = Some(Box::new(move |node_any| {
            r.fill(FormHandle::new(node_any, OPS));
        }));
        self
    }
}

impl IntoElement for FormBound {
    fn into_element(self) -> Element {
        item(
            FormPrim {
                on_submit: self.on_submit,
                style: RefCell::new(self.style),
                ref_fill: RefCell::new(self.ref_fill),
            },
            self.children,
        )
    }
}

/// Mirror of the old core's `From<Bound<H>> for Element`.
impl From<FormBound> for Element {
    fn from(b: FormBound) -> Element {
        b.into_element()
    }
}

// ============================================================================
// `ui!` dispatch — type alias + manual BuildElement impl (the table
// precedent; `#[component]` is old-core-only).
// ============================================================================

/// `ui!` tag alias for the form container — `ui! { Form(..) { … } }`
/// resolves to this type and dispatches through [`BuildElement`]. The
/// tag form yields an `Element` (the handle is dropped) — to bind a
/// `Ref<FormHandle>`, use the fn-call form: `form(props).bind(r)`.
pub type Form = FormProps;

impl BuildElement for FormProps {
    fn build(self) -> Element {
        form(self).into_element()
    }
}

/// One-stop import: same names as the old-core prelude.
pub mod prelude {
    pub use super::{form, Form, FormBuilder, FormHandle, FormProps};
}

// ============================================================================
// Handlers + registration seam
// ============================================================================

/// Shared mount tail — the old walker's `external.rs` sequence AFTER
/// children have been realized into the node: author style → ref fill
/// (type-erased node clone) → scope-tied `release_external`.
fn finish_mount<H>(backend: &Rc<RefCell<H>>, node: &H::Node, prim: &FormPrim)
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

/// Placeholder handler for hosts with no real form element — the
/// External degradation path, EXTENDED with children (the old walker
/// realized an External's children into the created node regardless of
/// whether a handler was registered — `walker/external.rs`:
/// create → children → style → ref fill → cleanup). Behaviorally the
/// old iOS/Android passthrough-container posture.
#[cfg(not(target_arch = "wasm32"))]
fn mount_placeholder<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<FormPrim>,
    children: Vec<Element>,
) -> H::Node
where
    H: ExternalOps + StyleServices,
{
    let backend = cx.backend().clone();
    // Payload parity with the old core: a `Rc<FormProps>` carrying
    // `on_submit` with the children already moved out.
    let payload: Rc<dyn Any> = Rc::new(FormProps {
        on_submit: prim.on_submit.clone(),
        children: Vec::new(),
    });
    let mut node = backend.borrow_mut().create_external(
        std::any::TypeId::of::<FormProps>(),
        std::any::type_name::<FormProps>(),
        &payload,
        &runtime_core::accessibility::AccessibilityProps::default(),
    );
    cx.realize_children_into(&mut node, children);
    finish_mount(&backend, &node, prim);
    node
}

/// Register the form payload handler on a scene registry. Pass this as
/// the boot registration seam (the `register` argument of
/// `backend_web::newcore::start_in` / `backend_ssr::newcore::
/// render_path_with`).
#[cfg(not(target_arch = "wasm32"))]
pub fn register<H>(registry: &mut Registry<H>)
where
    H: ExternalOps + StyleServices + 'static,
{
    registry.register::<FormPrim, _>(mount_placeholder::<H>);
}

/// Register the form payload handler on the web backend's scene
/// registry — the real `<form>` renderer.
#[cfg(target_arch = "wasm32")]
pub fn register(registry: &mut Registry<backend_web::WebBackend>) {
    registry.register::<FormPrim, _>(web_glue::mount_form_web);
}

// ============================================================================
// Web glue (wasm32): the old web handler, call-for-call, over the
// scene contract.
// ============================================================================

#[cfg(target_arch = "wasm32")]
mod web_glue {
    use super::*;
    use backend_web::WebBackend;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use web_sys::Event;

    pub(super) static OPS: &dyn FormOps = &WebFormOps;

    struct WebFormOps;
    impl FormOps for WebFormOps {
        fn submit(&self, node: &dyn Any) {
            // Shared with the old-core leg (web_util) so the two cores'
            // imperative submit can't drift.
            crate::web_util::request_submit(node);
        }
    }

    /// Per-form owned state — the submit listener closure stays alive
    /// here so the browser's event-target table keeps a valid callback
    /// to fire. Detaching the form drops the `Rc` (held via a JS
    /// reflect property), which drops the closure. (Old web handler,
    /// verbatim.)
    struct FormState {
        submit_listener: Option<Closure<dyn FnMut(Event)>>,
    }

    pub(super) fn mount_form_web(
        cx: &mut MountCx<'_, WebBackend>,
        prim: &Rc<FormPrim>,
        children: Vec<Element>,
    ) -> web_sys::Node {
        let backend = cx.backend().clone();
        let document = web_sys::window()
            .expect("no window")
            .document()
            .expect("no document");
        let form = document
            .create_element("form")
            .expect("create_element(form) failed");
        let _ = form.set_attribute("data-external-kind", "form::FormProps");

        let state = Rc::new(RefCell::new(FormState {
            submit_listener: None,
        }));

        if let Some(cb) = prim.on_submit.clone() {
            // `preventDefault()` is mandatory: without it the browser
            // performs the default GET/POST navigation and reloads the
            // SPA, tearing down the framework runtime. idealyst forms
            // carry their data in signals, not FormData, so the default
            // action is never wanted.
            let closure: Closure<dyn FnMut(Event)> = Closure::new(move |ev: Event| {
                ev.prevent_default();
                cb();
                // External web glue must call `schedule_flush` after the
                // author callback returns — this raw DOM listener is one
                // of the "residual surfaces" named in
                // `backend-web/src/newcore.rs`'s module docs: it is not
                // wrapped by the backend's capability impls, so a signal
                // write inside `on_submit` would stay staged in the
                // world until some unrelated event flushed it.
                backend_web::newcore::schedule_flush();
            });
            let _ = form
                .add_event_listener_with_callback("submit", closure.as_ref().unchecked_ref());
            state.borrow_mut().submit_listener = Some(closure);
        }

        // Stash the state Rc on the form so its lifetime matches the
        // form's (old web handler, verbatim).
        let raw = Rc::into_raw(state);
        let _ = js_sys::Reflect::set(
            form.as_ref(),
            &JsValue::from_str("__form_state"),
            &JsValue::from_f64(raw as usize as f64),
        );

        // Children BEFORE style — the old walker's `external.rs` order.
        // They become real DOM descendants of the `<form>`, which is
        // what makes browser autofill + submit-on-enter work.
        let mut node: web_sys::Node = form.into();
        cx.realize_children_into(&mut node, children);
        finish_mount(&backend, &node, prim);
        node
    }
}
