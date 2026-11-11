//! View creation, raw input channels, and the pressable container.

use std::rc::Rc;

use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::{
    FileDropHandler, FocusWithinHandler, HoverHandler, TouchHandler, TouchId, ViewHandle,
    WheelHandler,
};
use runtime_scene::Host;

use super::noop;

/// The `view` primitive — the container every other default lowers to
/// (via supertrait bounds, every default that degrades to a plain
/// container). Mounted by `handlers/view.rs`.
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
/// hover, keyboard focus-within, OS file drag-and-drop, and the
/// focus-preservation mark.
/// Third-party nodes get `on_touch`/`on_hover` too — which is why these
/// live apart from [`ViewOps`].
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

    /// Wire `handler` to keyboard focus entering (`true`) / leaving
    /// (`false`) `node` or any descendant — focus-within, gated on
    /// keyboard focus (`:focus-visible`), see
    /// [`runtime_shared::focus`]. Called for a `view` / `pressable` with
    /// an `on_focus`. A backend may report raw transitions (repeats,
    /// a `false` while nothing was focused): the vocabulary dedups.
    ///
    /// Default: no-op, for a backend with no keyboard-focus model.
    #[allow(unused_variables)]
    fn install_focus_handler(&mut self, node: &Self::Node, handler: FocusWithinHandler) {
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

/// The `pressable` primitive — a tappable container. `: ViewOps` because
/// the default degrades to a plain (non-tappable) view.
pub trait PressableOps: ViewOps {
    /// Tappable container with a click handler attached.
    #[allow(unused_variables)]
    fn create_pressable(&mut self, on_click: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> Self::Node {
        self.create_view(a11y)
    }

    /// Tappable container whose `on_press` fires as the primary pointer
    /// goes DOWN on it ([`PressActivation::Down`](runtime_shared::PressActivation)),
    /// not on release. Everything else matches [`create_pressable`]: the
    /// same node kind, focusability, a11y and keyboard activation.
    ///
    /// The contract every implementation keeps:
    /// - a primary press fires `on_press` once, at the down;
    /// - the rest of that press (moves, the release, wherever it lands)
    ///   fires nothing more and does not reach ancestors;
    /// - a secondary press fires nothing;
    /// - keyboard / assistive-tech activation and
    ///   [`PressableOps::click`](runtime_shared::PressableOps::click) fire
    ///   `on_press` once, as for a release-mode pressable.
    ///
    /// A touch backend also reports the held FINGER's travel after the down
    /// (`runtime_shared::held_press::report`: each move, then the lift or
    /// cancel), so something the press opened can follow it. Mouse presses
    /// need nothing extra; `pointer_input` already carries them app-wide.
    ///
    /// No default fallback to release: a backend that silently fired on
    /// release would render the same tree with different behaviour, so a
    /// backend without support panics at mount instead.
    ///
    /// [`create_pressable`]: PressableOps::create_pressable
    #[allow(unused_variables)]
    fn create_pressable_on_down(&mut self, on_press: Rc<dyn Fn()>, a11y: &AccessibilityProps) -> Self::Node {
        panic!(
            "this backend does not implement `PressableOps::create_pressable_on_down`, \
             so a pressable with `PressActivation::Down` cannot mount on it"
        )
    }

    /// Imperative-ref handle for a pressable. Default: no-op.
    #[allow(unused_variables)]
    fn make_pressable_handle(&self, node: &Self::Node) -> runtime_shared::PressableHandle {
        runtime_shared::PressableHandle::new(Rc::new(()), &noop::NoopPressableOps)
    }
}
