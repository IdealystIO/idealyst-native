//! Third-party (External) primitives and document-backed structural
//! affordances.

use std::any::Any;
use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_scene::Host;

use super::ViewOps;

/// Registry-dispatched third-party primitives (`Element::External`
/// today; the universal mount path once the scene Registry replaces the
/// walker). Serves `walker/external.rs` + `walker/cleanup.rs`. Kept
/// through the transition — at P7 this dissolves into the Registry
/// handler contract itself.
///
/// Also home of [`missing_primitive_placeholder`](Self::missing_primitive_placeholder):
/// every `create_*` default that renders the "backend compiled without
/// `prim-*`" box declares `: ExternalOps` for it, preserving the frozen
/// degradation path (a missing primitive renders as an External
/// placeholder, uniformly, instead of panicking).
pub trait ExternalOps: Host {
    /// Create a third-party node. `type_id` drives registry dispatch;
    /// `type_name` is for diagnostics only.
    #[allow(unused_variables)]
    fn create_external(
        &mut self,
        type_id: std::any::TypeId,
        type_name: &'static str,
        payload: &Rc<dyn Any>,
        a11y: &AccessibilityProps,
    ) -> Self::Node {
        unimplemented!(
            "create_external not implemented for this backend (external primitive: {})",
            type_name
        )
    }

    /// Tear down an external primitive's backend-side state.
    #[allow(unused_variables)]
    fn release_external(&mut self, node: &Self::Node) {
        // default no-op
    }

    /// Fallback node for a primitive family this backend was compiled
    /// without (`prim-*` feature mismatch): a visible, labeled
    /// "unsupported" box via [`create_external`](Self::create_external).
    /// Not meant to be overridden. The local `MissingPrimitive` TypeId
    /// differs from runtime-core's private one, but both are unknown to
    /// every registry, so dispatch behavior is identical.
    #[doc(hidden)]
    fn missing_primitive_placeholder(&mut self, label: &'static str) -> Self::Node {
        struct MissingPrimitive;
        let payload: Rc<dyn Any> = Rc::new(());
        self.create_external(
            std::any::TypeId::of::<MissingPrimitive>(),
            label,
            &payload,
            &AccessibilityProps::default(),
        )
    }
}

/// Document-backed structural affordances: real HTML tags, id/class/
/// inline-declaration stamping, and raw CSS registration. Web/SSR
/// implement these; native backends keep the no-ops. Serves
/// `walker/navigator.rs` (structural hydration classes), the lazy
/// chunk loader (`attach_html_id`), and third-party External handlers
/// that build cross-backend DOM structure. `: ViewOps` because the
/// frozen `create_element` default falls back to a plain container.
pub trait DocumentOps: ViewOps {
    /// Create a structural element with an explicit HTML-ish tag.
    /// Backends with no tag concept fall back to a plain view.
    #[allow(unused_variables)]
    fn create_element(&mut self, tag: &str) -> Self::Node {
        self.create_view(&AccessibilityProps::default())
    }

    /// Stamp a stable, `getElementById`-findable id on `node`.
    #[allow(unused_variables)]
    fn attach_html_id(&self, node: &Self::Node, id: &str) {}

    /// Stamp a structural class name on `node` (hydration markers,
    /// navigator chrome classes).
    #[allow(unused_variables)]
    fn attach_html_class(&self, node: &Self::Node, class: &str) {}

    /// Set an inline CSS custom-property / declaration on `node`.
    #[allow(unused_variables)]
    fn attach_html_style(&self, node: &Self::Node, prop: &str, value: &str) {}

    /// Register a raw CSS stylesheet to ship once (paired with
    /// [`attach_html_class`](Self::attach_html_class)).
    #[allow(unused_variables)]
    fn register_raw_css(&mut self, css: &str) {
        // default: no-op
    }
}
