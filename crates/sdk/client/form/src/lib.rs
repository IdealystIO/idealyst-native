//! Third-party `Form` SDK for the idealyst framework.
//!
//! Provides a `Form` container: a real `<form>` on web (submit-on-enter,
//! autofill grouping, `preventDefault()`-guarded `on_submit`), a plain
//! passthrough container on native (submission is the author's submit
//! button calling `on_submit`).
//!
//! # Implementation
//!
//! The scene [`Registry`] is the runtime's unified primitive==external
//! contract, so the SDK registers a payload handler there:
//!
//! - **Web (wasm32)** — [`register`] installs a `WebBackend`-concrete
//!   handler: one real `<form>` element, the native `submit` event wired
//!   to `preventDefault()` + the author `on_submit` (plus the mandatory
//!   post-callback `schedule_flush` — see the handler), the
//!   `__form_state` reflect-slot keeping the listener closure alive,
//!   children realized as real DOM descendants of the `<form>` (that's
//!   what makes browser autofill + submit-on-enter work), author style,
//!   ref fill with the web ops (`requestSubmit`, in [`web_util`]).
//! - **Everywhere else** — [`register`] installs the
//!   External-placeholder path ([`ExternalOps::create_external`]) with
//!   the children realized INTO the returned node (create → children →
//!   style → ref fill → cleanup). Behaviorally this is the
//!   passthrough-container posture: children lay out inside a plain
//!   container and submission is the author's button calling
//!   `on_submit`. There is no dedicated iOS `UIView` / Android
//!   `FrameLayout` renderer; a native port would also have to route
//!   author callbacks through the platform's post-dispatch flush seam
//!   (the `schedule_flush` residual noted in
//!   `backend-web/src/newcore.rs`'s module docs applies to every
//!   external glue that runs author code from a raw platform event).
//!
//! # Usage
//!
//! ```ignore
//! use form::prelude::*;       // brings in `Form`, `form`, `FormProps`
//! use idea_ui::prelude::*;    // Button, TextInput, …
//!
//! // App bootstrap: the boot entry's `register` argument IS the
//! // registration seam (one line per third-party SDK).
//! backend_web::newcore::start_in("#app", form::register, app);
//!
//! // The submit action is a plain closure that reads your field
//! // signals — it is NOT fed by the DOM's FormData. Build it once and
//! // share the `Rc`: hand it to the form (web Enter-to-submit) AND to
//! // your submit button (the universal trigger).
//! let name = signal(String::new());
//! let on_submit: std::rc::Rc<dyn Fn()> = {
//!     let name = name.clone();
//!     std::rc::Rc::new(move || log::info!("submit: {}", name.get()))
//! };
//!
//! ui! {
//!     Form(on_submit = Some(on_submit.clone())) {
//!         TextInput(value = name.clone())
//!         Button(label = "Save", on_click = on_submit.clone())
//!     }
//! }
//! ```
//!
//! An UNREGISTERED payload panics at realize (the scene contract), so a
//! missed `register` fails loud.
//!
//! # Why this is an SDK and not a core primitive
//!
//! A form has no convergent cross-platform behavior to put behind the
//! host capability set: on web `<form>` is a real element
//! (submit-on-enter, autofill grouping, FormData), while iOS/Android
//! have NO form construct — their form affordances (autofill,
//! return-key submit) live per-field on the inputs, not on a container.
//! So `Form` is an opinionated SDK on the external-element contract
//! (with children):
//!   * web    -> a real `<form>` wrapping the inputs as DOM descendants,
//!              with the native `submit` event wired to `on_submit`
//!              after `preventDefault()`.
//!   * native -> a plain passthrough container; submission is triggered
//!              by the author's submit button calling `on_submit`.
//!
//! # Why `on_submit` translates across platforms
//!
//! It's a triggered *action* (uniform closure), separated from its
//! *trigger* (platform-idiomatic) and its *data* (uniform signals):
//!
//! - **Web** — the handler wires the real `<form>`'s `submit` event,
//!   calls `preventDefault()` (idealyst apps don't POST form-encoded
//!   data — the browser must not navigate/reload), then invokes
//!   `on_submit`. Free Enter-to-submit, and autofill works because the
//!   inputs are real DOM descendants of the `<form>`.
//! - **Native** — there is no form `submit` event, so the handler is a
//!   passthrough container and submission is fired by the author's
//!   submit button calling `on_submit` directly. (Keyboard return /
//!   IME-action submit is a *field-level* affordance and belongs on the
//!   input.)
#![deny(missing_docs)]

// Shared wasm32 helpers (pure DOM ops on the mounted `<form>`, no core
// types).
#[cfg(target_arch = "wasm32")]
pub(crate) mod web_util;

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::Ref;
use runtime_scene::{item, Element, Host, MountCx, Registry};
use runtime_vocabulary::caps::ExternalOps;
use runtime_vocabulary::glue::{BuildElement, IntoElement};
use runtime_vocabulary::style_attach::{
    attach_style, on_teardown, IntoStyleProp, StyleProp, StyleServices,
};

// ============================================================================
// Public API surface
// ============================================================================

/// Author-supplied props for a `Form`. `on_submit` rides the scene item
/// payload and is read back by the registered handler.
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
// Handle + ops trait
// ============================================================================

/// Typed handle to a mounted `Form`. Filled at mount time when the
/// author chained [`FormBuilder::bind`]; user code receives the handle
/// through `Ref::with`.
#[derive(Clone)]
pub struct FormHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn FormOps,
}

/// Pointer identity on the NODE — a `FormHandle` names one mounted
/// `<form>`, so clones of it are equal and handles onto two different
/// forms never are.
///
/// `node` is a type-erased native element behind `Rc<dyn Any>`: there is
/// nothing to compare but the address, and the address is the right thing
/// to compare — "same form?" is precisely the question. `ops` is
/// deliberately NOT part of the comparison; it is the backend's single
/// `&'static` vtable, identical for every handle on a given target, so it
/// carries no information about which form this is.
///
/// Needed because `Signal<T>` is bounded on `T: PartialEq` at creation and
/// `get`, not just on the guarded `set` — an author who stashes the bound
/// handle in state to submit the form from elsewhere cannot add the impl
/// themselves (orphan rule). Mirrors `MediaStream`.
impl PartialEq for FormHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.node, &other.node)
    }
}

impl Eq for FormHandle {}

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

/// Imperative-ops dispatch. The active target's `OPS` static supplies
/// the impl.
pub trait FormOps: Sync {
    /// Submit the form represented by `node`. The web impl downcasts to
    /// the concrete `<form>` element and triggers submission; other
    /// targets keep the default no-op since there's no form machinery.
    fn submit(&self, _node: &dyn Any) {}
}

/// Fallback ops used on targets with no `Form` impl (every non-web
/// target — the placeholder posture).
pub struct UnsupportedOps;
impl FormOps for UnsupportedOps {}

#[cfg(target_arch = "wasm32")]
static OPS: &dyn FormOps = web_glue::OPS;
#[cfg(not(target_arch = "wasm32"))]
static OPS: &dyn FormOps = &UnsupportedOps;

// ============================================================================
// Payload + builder — the `.with_style(…)` / `.bind(…)` chain then
// element coercion.
// ============================================================================

/// Scene payload for the `Form` item. Single-take slots (the vocabulary
/// `PrimCell` discipline, inlined): the scene hands the handler a
/// shared `&Rc<Self>`, but the style/ref-fill must move at mount.
/// Children do NOT ride the payload — they ride the scene item's
/// children slot and the handler parents them.
struct FormPrim {
    on_submit: Option<Rc<dyn Fn()>>,
    style: RefCell<Option<StyleProp>>,
    ref_fill: RefCell<Option<Box<dyn FnOnce(Rc<dyn Any>)>>>,
}

/// Author-side builder returned by [`form`].
pub struct FormBound {
    on_submit: Option<Rc<dyn Fn()>>,
    children: Vec<Element>,
    style: Option<StyleProp>,
    ref_fill: Option<Box<dyn FnOnce(Rc<dyn Any>)>>,
}

/// Build a `Form` container programmatically. The PascalCase [`Form`]
/// tag delegates here; use this fn-call form (`form(props).bind(r)`)
/// when you need the handle.
pub fn form(mut props: FormProps) -> FormBound {
    // Children ride the scene item's children slot (the handler parents
    // them); the payload only needs to carry `on_submit`.
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
    /// the placeholder container elsewhere.
    pub fn with_style(mut self, style: impl IntoStyleProp) -> Self {
        self.style = Some(style.into_style_prop());
        self
    }
}

/// Adds `.bind(r)` so `use form::prelude::*` brings the imperative
/// binding into scope.
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

/// Element coercion for the fn-call form.
impl From<FormBound> for Element {
    fn from(b: FormBound) -> Element {
        b.into_element()
    }
}

// ============================================================================
// `ui!` dispatch — type alias + manual BuildElement impl (a container
// whose children move out of the props needs the hand-rolled impl).
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

/// One-stop import for the author-facing names.
pub mod prelude {
    pub use super::{form, Form, FormBuilder, FormHandle, FormProps};
}

// ============================================================================
// Handlers + registration seam
// ============================================================================

/// Shared mount tail, run AFTER children have been realized into the
/// node: author style → ref fill (type-erased node clone) → scope-tied
/// `release_external`.
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
/// External degradation path, EXTENDED with children (create →
/// children → style → ref fill → cleanup): the passthrough-container
/// posture.
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
    // The payload handed to the host is a `Rc<FormProps>` carrying
    // `on_submit` with the children already moved out.
    let payload: Rc<dyn Any> = Rc::new(FormProps {
        on_submit: prim.on_submit.clone(),
        children: Vec::new(),
    });
    let mut node = backend.borrow_mut().create_external(
        std::any::TypeId::of::<FormProps>(),
        std::any::type_name::<FormProps>(),
        &payload,
        &runtime_shared::accessibility::AccessibilityProps::default(),
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
        registry.defer::<FormPrim>();
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
        registry.register_deferred::<FormPrim, _>(web_glue::mount_form_web);
    });
}

/// Non-web stub — see the wasm32 [`register_from_chunk`].
#[cfg(not(target_arch = "wasm32"))]
pub fn register_from_chunk() {}

// ============================================================================
// Web glue (wasm32): the real `<form>` renderer over the scene
// contract.
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
            crate::web_util::request_submit(node);
        }
    }

    /// Per-form owned state — the submit listener closure stays alive
    /// here so the browser's event-target table keeps a valid callback
    /// to fire. Detaching the form drops the `Rc` (held via a JS
    /// reflect property), which drops the closure.
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
        // form's.
        let raw = Rc::into_raw(state);
        let _ = js_sys::Reflect::set(
            form.as_ref(),
            &JsValue::from_str("__form_state"),
            &JsValue::from_f64(raw as usize as f64),
        );

        // Children BEFORE style. They become real DOM descendants of
        // the `<form>`, which is what makes browser autofill +
        // submit-on-enter work.
        let mut node: web_sys::Node = form.into();
        cx.realize_children_into(&mut node, children);
        finish_mount(&backend, &node, prim);
        node
    }
}
