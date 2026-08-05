//! View creation, raw input channels, and the pressable container.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::{
    FileDropHandler, HoverHandler, TouchHandler, TouchId, ViewHandle, WheelHandler,
};
use runtime_scene::Host;

use super::noop;

/// The `view` primitive — the container every other default lowers to.
/// Serves `walker/view.rs` (and, via supertrait bounds, every default
/// that degrades to a plain container).
pub trait ViewOps: Host {
    /// Create a plain container node.
    fn create_view(&mut self, a11y: &AccessibilityProps) -> Self::Node;

    /// Imperative-ref handle for a view. Default: type-correct no-op.
    #[allow(unused_variables)]
    fn make_view_handle(&self, node: &Self::Node) -> ViewHandle {
        ViewHandle::new(Rc::new(()), &noop::NoopViewOps)
    }
}

/// Raw input channels installed on already-created nodes: touch, wheel,
/// hover, OS file drag-and-drop, and the focus-preservation mark.
/// Serves `walker/view.rs`, `walker/pressable.rs`
/// (`mark_preserves_focus`), and `walker/external.rs` (externals get
/// `on_touch`/`on_hover` too — which is why these live apart from
/// [`ViewOps`]).
pub trait InputOps: Host {
    /// Wire `handler` to the platform's raw touch delivery for `node`.
    #[allow(unused_variables)]
    fn install_touch_handler(&mut self, node: &Self::Node, handler: TouchHandler) {
        // default: no-op
    }

    /// A handler claimed this touch; suppress competing native
    /// consumers (scroll containers, system gestures).
    #[allow(unused_variables)]
    fn claim_touch(&mut self, node: &Self::Node, touch_id: TouchId) {
        // default: no-op
    }

    /// Wire `handler` to the desktop wheel / magnify source.
    #[allow(unused_variables)]
    fn install_wheel_handler(&mut self, node: &Self::Node, handler: WheelHandler) {
        // default: no-op
    }

    /// Wire `handler` to pointer-enter/leave (`true` on enter).
    #[allow(unused_variables)]
    fn install_hover_handler(&mut self, node: &Self::Node, handler: HoverHandler) {
        // default: no-op
    }

    /// Mark `node`'s subtree as focus-preserving: a press inside it
    /// must not blur the focused text input (combobox capability).
    #[allow(unused_variables)]
    fn mark_preserves_focus(&mut self, node: &Self::Node) {
        // default: no-op
    }

    /// Wire `handler` to the OS file drag-and-drop machinery.
    #[allow(unused_variables)]
    fn install_file_drop_handler(&mut self, node: &Self::Node, handler: FileDropHandler) {
        // default: no-op
    }
}

/// The `pressable` primitive — a tappable container. Serves
/// `walker/pressable.rs`. `: ViewOps` because the frozen default
/// degrades to a plain (non-tappable) view.
pub trait PressableOps: ViewOps {
    /// Tappable container with a click handler attached.
    #[allow(unused_variables)]
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> Self::Node {
        self.create_view(a11y)
    }

    /// Imperative-ref handle for a pressable. Default: no-op.
    #[allow(unused_variables)]
    fn make_pressable_handle(&self, node: &Self::Node) -> runtime_shared::PressableHandle {
        runtime_shared::PressableHandle::new(Rc::new(()), &noop::NoopPressableOps)
    }
}
