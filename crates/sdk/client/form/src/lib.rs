//! Third-party `Form` SDK for the idealyst framework.
//!
//! Provides a `Form` container backed by the framework's
//! `Element::External` extension mechanism (with children). `Form` is a
//! library component in the idea-ui mould: a snake-case `form(props)`
//! function plus a `#[macro_export]`ed `Form!` invocation macro, so it
//! reads as a first-class element inside `ui!`/`jsx!`.
//!
//! # Usage
//!
//! ```ignore
//! use form::prelude::*;       // brings in `Form!`, `form`, `FormProps`
//! use idea_ui::prelude::*;    // Button, TextInput, …
//!
//! // App bootstrap (one line per third-party SDK):
//! let mut backend = WebBackend::new("#app");
//! form::register(&mut backend);
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
//! # Why this is an SDK and not a core primitive
//!
//! A form has no convergent cross-platform behavior to put behind the
//! Backend trait: on web `<form>` is a real element (submit-on-enter,
//! autofill grouping, FormData), while iOS/Android have NO form
//! construct — their form affordances (autofill, return-key submit)
//! live per-field on the inputs, not on a container. So `Form` is an
//! opinionated SDK on `Element::External` (with children):
//!   * web    → a real `<form>` wrapping the inputs as DOM descendants,
//!              with the native `submit` event wired to `on_submit`
//!              after `preventDefault()`.
//!   * native → a plain passthrough container; submission is triggered
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

use runtime_core::{Bound, Element, IdealystSchema, Ref, RefFill};
use std::any::{Any, TypeId};
use std::rc::Rc;

// ============================================================================
// Public API surface
// ============================================================================

/// Author-supplied props for a `Form`. Owned by the SDK — the framework
/// type-erases this behind `Element::External` and hands it back to the
/// registered backend handler on mount.
#[derive(Default, IdealystSchema)]
pub struct FormProps {
    /// The submit action. On web it fires on the native `<form>` submit
    /// event (Enter in a field or a `type="submit"` descendant) AFTER
    /// `preventDefault()`. On native it is invoked by the author's
    /// submit button. Read your field signals inside this closure.
    ///
    /// `Rc` (not `Box`) because the framework hands the handler a
    /// `Rc<FormProps>` and the handler can only borrow — it clones the
    /// `Rc` into the event listener. Share the same `Rc` with your
    /// submit button so one closure covers every backend.
    pub on_submit: Option<Rc<dyn Fn()>>,
    /// Form contents. The framework parents these INTO the backend node
    /// the handler returns: on web they become real DOM descendants of
    /// the `<form>` (required for autofill + submit-on-enter); on native
    /// they're laid out inside the passthrough container. Populated for
    /// you by the `ui!`/`jsx!` children block.
    pub children: Vec<Element>,
}

// ============================================================================
// Handle + ops trait
// ============================================================================

/// Typed handle to a mounted `Form`. Filled by `Ref::fill` after the
/// form mounts; hold a `Ref<FormHandle>` at the call site and reach
/// imperative ops via `r.with(|h| h.submit())`.
#[derive(Clone)]
pub struct FormHandle {
    node: Rc<dyn Any>,
    ops: &'static dyn FormOps,
}

impl FormHandle {
    /// Wrap a type-erased native form node + its backend ops vtable.
    /// Called by the backend's `RefFill` after the form mounts; you
    /// don't construct this directly.
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

/// Imperative-ops dispatch. The web impl downcasts `node` to the
/// concrete `<form>` element; native impls keep the default no-op
/// because there's no form-submit machinery to drive.
///
/// `Sync` bound: the trait object lives in a `static OPS` slot per
/// backend module, which Rust requires to be `Sync`. The ZST impls are
/// trivially `Sync`.
pub trait FormOps: Sync {
    /// Submit the form represented by `node`. The web impl downcasts to
    /// the concrete `<form>` element and triggers submission; native
    /// impls leave the default no-op since there's no form machinery.
    fn submit(&self, _node: &dyn Any) {}
}

/// Fallback ops for targets with no `Form` impl. The framework's
/// `External` placeholder is what renders at runtime.
pub struct UnsupportedOps;
impl FormOps for UnsupportedOps {}

// ============================================================================
// Constructor + invocation macro
// ============================================================================

/// Build a `Form` container. Snake-case because the PascalCase tag
/// (`Form`) is the invocation macro; `ui! { Form(..) { .. } }` lowers to
/// `Form!(..)` which calls this. Returns a typed `Bound<FormHandle>` so
/// a trailing `.bind(..)` chain type-checks against `Ref<FormHandle>`.
pub fn form(mut props: FormProps) -> Bound<FormHandle> {
    // Children parent into the backend node (the External slot); the
    // payload only needs to carry `on_submit`, so move children out
    // rather than ship a second copy inside the payload.
    let children = std::mem::take(&mut props.children);
    Bound::new(Element::External {
        type_id: TypeId::of::<FormProps>(),
        type_name: std::any::type_name::<FormProps>(),
        payload: Rc::new(props) as Rc<dyn Any>,
        children,
        style: None,
        ref_fill: None,
        accessibility: runtime_core::accessibility::AccessibilityProps::default(),
    })
}

/// `ui!`/`jsx!` invocation macro for `Form`. Hand-written +
/// `#[macro_export]`ed (with `$crate::` paths) because `#[component]`
/// only produces a crate-local macro, and this SDK is a library whose
/// `Form!` must be callable from consumer crates. `children` is just a
/// `FormProps` field, so the emitter's `children = { Vec<Element> }`
/// lowering needs no special arm. Every field is coerced via `.into()`,
/// matching the framework's uniform invocation-macro convention.
#[macro_export]
#[doc(hidden)]
macro_rules! Form {
    () => {
        $crate::form(<$crate::FormProps as ::core::default::Default>::default())
    };
    ( $( $field:ident = $value:expr ),+ $(,)? ) => {
        $crate::form($crate::FormProps {
            $( $field: ($value).into(), )+
            ..<$crate::FormProps as ::core::default::Default>::default()
        })
    };
}

/// Builder methods on `Bound<FormHandle>`. An extension trait because
/// the orphan rule blocks an inherent `impl Bound<FormHandle>` here
/// (`Bound` is foreign). Usable as a trailing `ui!` chain:
/// `Form(..) { .. }.bind(r)`.
pub trait FormBuilder {
    /// Bind a `Ref<FormHandle>` for imperative access (e.g.
    /// `r.with(|h| h.submit())`).
    fn bind(self, r: Ref<FormHandle>) -> Self;
}

impl FormBuilder for Bound<FormHandle> {
    fn bind(mut self, r: Ref<FormHandle>) -> Self {
        if let Element::External { ref_fill, .. } = self.primitive_mut() {
            *ref_fill = Some(RefFill::External(Box::new(move |node_any| {
                r.fill(FormHandle::new(node_any, OPS));
            })));
        }
        self
    }
}

/// One-stop import: `use form::prelude::*;` brings in the `Form!` macro,
/// the `form` constructor, the props struct, the handle type, and the
/// `.bind(..)` builder trait.
pub mod prelude {
    pub use super::{form, FormBuilder, FormHandle, FormProps};
    pub use crate::Form;
}

// ============================================================================
// Backend selector
// ============================================================================

// Each platform module exposes `pub fn register(&mut <Backend>)` and a
// `pub static OPS: &dyn FormOps`. Only one compiles per target via cfg.

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::register;
#[cfg(target_arch = "wasm32")]
static OPS: &dyn FormOps = web::OPS;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub use android::register;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
static OPS: &dyn FormOps = android::OPS;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub use ios::register;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
static OPS: &dyn FormOps = ios::OPS;

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
mod fallback {
    use runtime_core::Backend;

    /// No-op register for unsupported targets. The framework's External
    /// placeholder shows up at runtime to make the missing binding
    /// obvious.
    pub fn register<B: Backend>(_backend: &mut B) {}
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
pub use fallback::register;

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")))]
static OPS: &dyn FormOps = &UnsupportedOps;

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_core::text;

    /// `form(..)` lowers to `Element::External` keyed by `FormProps`'s
    /// TypeId (so backend handlers dispatch to it) and starts childless.
    #[test]
    fn form_builds_external_keyed_by_form_props() {
        let el: Element = form(FormProps::default()).into();
        match el {
            Element::External { type_id, type_name, children, .. } => {
                assert_eq!(type_id, TypeId::of::<FormProps>());
                assert!(type_name.contains("FormProps"));
                assert!(children.is_empty(), "no children by default");
            }
            _ => panic!("form must lower to Element::External"),
        }
    }

    /// The `children` prop moves into the External's children slot —
    /// these are what the framework parents into the `<form>` on web.
    #[test]
    fn children_prop_moves_into_external_slot() {
        let el: Element = form(FormProps {
            children: vec![text("a").into(), text("b").into()],
            ..Default::default()
        })
        .into();
        match el {
            Element::External { children, .. } => assert_eq!(children.len(), 2),
            _ => panic!("expected Element::External"),
        }
    }

    /// The `Form!` invocation macro (what `ui! { Form { .. } }` lowers
    /// to) builds the same External, coercing each field via `.into()`.
    #[test]
    fn form_macro_lowers_children() {
        let el: Element =
            crate::Form!(children = vec![text("a").into()]).into();
        match el {
            Element::External { type_id, children, .. } => {
                assert_eq!(type_id, TypeId::of::<FormProps>());
                assert_eq!(children.len(), 1);
            }
            _ => panic!("expected Element::External"),
        }
    }

    /// End-to-end: the real `ui!` macro lowers `Form { View {} }` to
    /// `Form!(children = { .. })` → `form(..)` → a `<form>` External
    /// whose children flow through unchanged. This is the author-facing
    /// path the SDK is meant to be used through.
    #[test]
    fn form_via_ui_macro() {
        use runtime_core::ui;
        let el: Element = ui! { Form { View {} View {} } };
        match el {
            Element::External { type_id, children, .. } => {
                assert_eq!(type_id, TypeId::of::<FormProps>());
                assert_eq!(children.len(), 2, "ui! children reach the External slot");
            }
            _ => panic!("ui! Form must build Element::External"),
        }
    }
}
