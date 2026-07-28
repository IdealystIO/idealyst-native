//! Text (plain, styled-runs, batched-id fast path, JS bindings) and
//! button capabilities.

use std::rc::Rc;

use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::icon::IconData;
use runtime_core::styled_text::{plain_text_of, TextRun};
use runtime_core::{Action, ButtonHandle, TextHandle};
use runtime_scene::Host;

use super::noop;

/// The `text` primitive: plain and styled-run creation/update, the
/// batched `*_by_id` FFI fast path, and backend-side reactive text
/// bindings. Serves `walker/text.rs` (and the theme-cohort re-realize
/// path for styled runs).
pub trait TextOps: Host {
    /// Create a text node with static content.
    fn create_text(&mut self, content: &str, a11y: &AccessibilityProps) -> Self::Node;

    /// Text node whose content is a list of inline-styled runs.
    /// Default lowers to [`create_text`](Self::create_text) with the
    /// concatenated plain text (styling degrades, words render).
    fn create_styled_text(&mut self, runs: &[TextRun], a11y: &AccessibilityProps) -> Self::Node {
        self.create_text(&plain_text_of(runs), a11y)
    }

    /// Re-realize a styled text node's runs (theme/token swap).
    /// Default mirrors `create_styled_text`: plain-concat via
    /// [`update_text`](Self::update_text).
    fn update_styled_text(&mut self, node: &Self::Node, runs: &[TextRun]) {
        self.update_text(node, &plain_text_of(runs));
    }

    /// Replace a text node's content.
    fn update_text(&mut self, node: &Self::Node, content: &str);

    /// Optional batched-text-update fast path: return `Some((node, id))`
    /// to receive subsequent updates via
    /// [`update_text_by_id`](Self::update_text_by_id).
    fn create_text_with_id(
        &mut self,
        _content: &str,
        _a11y: &AccessibilityProps,
    ) -> Option<(Self::Node, u32)> {
        None
    }

    /// Companion to [`create_text_with_id`](Self::create_text_with_id).
    /// Takes `String` by value so the caller's allocation moves into
    /// the backend's pending buffer.
    fn update_text_by_id(&mut self, _id: u32, _content: String) {
        unreachable!(
            "update_text_by_id called without a matching create_text_with_id override; \
             a backend that returns Some(_) from create_text_with_id must also override \
             update_text_by_id"
        )
    }

    /// Release an id assigned by [`create_text_with_id`](Self::create_text_with_id).
    fn release_text_id(&mut self, _id: u32) {}

    /// `true` if this backend implements
    /// [`register_reactive_text_binding`](Self::register_reactive_text_binding).
    fn supports_js_text_bindings(&self) -> bool {
        false
    }

    /// Register a pre-decomposed reactive text binding with the
    /// backend's own fan-out path (no Rust Effect per fire).
    fn register_reactive_text_binding(
        &mut self,
        _text_id: u32,
        _signal_ids: &[u64],
        _template_parts: &[&str],
        _initial_values: &[&str],
        _stringifiers: &[Rc<dyn Fn() -> String>],
    ) {
        unreachable!(
            "register_reactive_text_binding called without an override; \
             a backend that returns true from supports_js_text_bindings must \
             also override register_reactive_text_binding"
        )
    }

    /// Release a binding registered via
    /// [`register_reactive_text_binding`](Self::register_reactive_text_binding).
    fn release_reactive_text_binding(&mut self, _text_id: u32) {}

    /// NEW-CORE-ONLY channel (the string sibling of
    /// `StyleOps::notify_signal_value_js` — no old-core counterpart in
    /// the frozen `Backend` mirror): ship a `(signal_id, formatted
    /// value)` change to the backend's JS-side binding dispatcher. On
    /// the old core the arena's `Signal::set` fired the registered JS
    /// notifier itself; world signals have no write hook, so the text
    /// handler's per-signal notifier effect (see
    /// `style_attach::ensure_signal_notifier_installed`) delivers
    /// commits through this method instead. Default no-op — only
    /// backends returning `true` from
    /// [`supports_js_text_bindings`](Self::supports_js_text_bindings)
    /// need to override.
    fn notify_signal_text_js(&mut self, _signal_id: u64, _value: &str) {}

    /// Imperative-ref handle for a text node. Default: no-op.
    #[allow(unused_variables)]
    fn make_text_handle(&self, node: &Self::Node) -> TextHandle {
        TextHandle::new(Rc::new(()), &noop::NoopTextOps)
    }
}

/// The `button` primitive. Serves `walker/button.rs`. `: TextOps`
/// because the frozen `update_button_label` default lowers to
/// `update_text` (most platforms share the underlying set-text API).
pub trait ButtonOps: TextOps {
    /// Create a button with label, action, and optional icons.
    fn create_button(
        &mut self,
        label: &str,
        on_click: &Action,
        leading_icon: Option<&IconData>,
        trailing_icon: Option<&IconData>,
        a11y: &AccessibilityProps,
    ) -> Self::Node;

    /// Update a button's visible label (reactive label prop).
    #[allow(unused_variables)]
    fn update_button_label(&mut self, node: &Self::Node, label: &str) {
        self.update_text(node, label);
    }

    /// Imperative-ref handle for a button. Default: no-op.
    #[allow(unused_variables)]
    fn make_button_handle(&self, node: &Self::Node) -> ButtonHandle {
        ButtonHandle::new(Rc::new(()), &noop::NoopButtonOps)
    }
}
