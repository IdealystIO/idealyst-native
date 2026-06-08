//! iOS-specific `*Ops` impls — currently scoped to providing
//! viewport-relative rects so views can serve as overlay anchors
//! (`Popover`, `Select`, etc.).
//!
//! Each Ops impl is a zero-sized type with a `'static` instance. The
//! backend's `make_*_handle` methods reference the appropriate static
//! when constructing the handle.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2_foundation::{CGRect, NSObject, NSString};
use objc2_ui_kit::{UITextField, UITextView, UIView};
use std::any::Any;

use runtime_core::primitives::portal::ViewportRect;
use runtime_core::primitives::text_area::TextAreaOps;
use runtime_core::primitives::text_input::TextInputOps;
use runtime_core::{ButtonOps, LayoutSubscription, PressableOps, ViewOps};
use std::cell::RefCell;
use std::rc::Rc;

use crate::imp::IosNode;

thread_local! {
    /// Per-view `on_layout` callbacks, keyed by the UIView pointer (the
    /// same `usize` key as `view_to_layout`). Fired from `apply_frames`
    /// after a view's frame changes — the UIKit analog of the web
    /// `ResizeObserver`, which is how a `.container()` view's inline-size
    /// signal gets fed on iOS. Main-thread only, so a thread-local is safe.
    static LAYOUT_SUBS: RefCell<Vec<(usize, Rc<dyn Fn(f32, f32)>)>> =
        const { RefCell::new(Vec::new()) };
}

/// Fire every `on_layout` callback registered for `view_key` with the
/// view's resolved inline-size (`w`) and block-size (`h`). The callbacks
/// change-guard, so re-firing at an unchanged size is a no-op.
pub(crate) fn fire_layout_for_view(view_key: usize, w: f32, h: f32) {
    let cbs: Vec<Rc<dyn Fn(f32, f32)>> = LAYOUT_SUBS.with(|m| {
        m.borrow()
            .iter()
            .filter(|(k, _)| *k == view_key)
            .map(|(_, c)| c.clone())
            .collect()
    });
    for c in cbs {
        c(w, h);
    }
}

/// Read the viewport-relative rect of an iOS node. Walks
/// `convertRect:toView:nil` to get window-coordinate bounds. Returns
/// the zero rect if the view isn't mounted in a window yet.
pub(crate) fn rect_of_node(node: &dyn Any) -> ViewportRect {
    let Some(ios_node) = node.downcast_ref::<IosNode>() else {
        return ViewportRect::default();
    };
    let view = ios_node.as_view();
    let bounds: CGRect = unsafe { msg_send![view, bounds] };

    // Convert bounds to window coordinates. `toView: nil` works on
    // iOS to get window-relative coords, but on some objc2 versions
    // passing nil for an id parameter is awkward — use the actual
    // window if available, otherwise return default.
    let window: Option<Retained<UIView>> = unsafe {
        let w: *mut UIView = msg_send![view, window];
        if w.is_null() {
            None
        } else {
            Retained::retain(w)
        }
    };
    let Some(window) = window else {
        return ViewportRect::default();
    };

    let frame_in_window: CGRect = unsafe {
        msg_send![view, convertRect: bounds, toView: &*window]
    };

    ViewportRect {
        x: frame_in_window.origin.x as f32,
        y: frame_in_window.origin.y as f32,
        width: frame_in_window.size.width as f32,
        height: frame_in_window.size.height as f32,
    }
}

// (CGRect comes from objc2_foundation and already implements `Encode`,
// so it can flow through msg_send! without a custom impl.)

// =============================================================================
// Ops impls — ZSTs, referenced via `&'static` from each handle.
// =============================================================================

pub(crate) struct IosButtonOps;
impl ButtonOps for IosButtonOps {
    fn click(&self, _node: &dyn Any) {
        // Programmatic click via handle not wired yet. Robot drives
        // clicks via the stored on_click action; user code rarely
        // calls handle.click() directly.
    }
    fn rect(&self, node: &dyn Any) -> ViewportRect {
        rect_of_node(node)
    }
}
pub(crate) static IOS_BUTTON_OPS: IosButtonOps = IosButtonOps;

pub(crate) struct IosPressableOps;
impl PressableOps for IosPressableOps {
    fn click(&self, _node: &dyn Any) {}
    fn rect(&self, node: &dyn Any) -> ViewportRect {
        rect_of_node(node)
    }
}
pub(crate) static IOS_PRESSABLE_OPS: IosPressableOps = IosPressableOps;

pub(crate) struct IosViewOps;
impl ViewOps for IosViewOps {
    fn subscribe_layout(
        &self,
        node: &dyn Any,
        callback: Box<dyn Fn(f32, f32)>,
    ) -> LayoutSubscription {
        let Some(ios_node) = node.downcast_ref::<IosNode>() else {
            return LayoutSubscription::noop();
        };
        // Same key derivation as `view_to_layout` / `IosNode::view_key`.
        let key = ios_node.as_view() as *const UIView as usize;
        let cb: Rc<dyn Fn(f32, f32)> = Rc::from(callback);
        let cb_id = Rc::as_ptr(&cb) as *const () as usize;
        LAYOUT_SUBS.with(|m| m.borrow_mut().push((key, cb)));
        LayoutSubscription::new(move || {
            LAYOUT_SUBS.with(|m| {
                m.borrow_mut()
                    .retain(|(k, c)| !(*k == key && Rc::as_ptr(c) as *const () as usize == cb_id))
            });
        })
    }

    fn rect(&self, node: &dyn Any) -> ViewportRect {
        rect_of_node(node)
    }
    fn frame(&self, node: &dyn Any) -> Option<ViewportRect> {
        let ios_node = node.downcast_ref::<IosNode>()?;
        let view = ios_node.as_view();
        let frame: CGRect = unsafe { msg_send![view, frame] };
        Some(ViewportRect {
            x: frame.origin.x as f32,
            y: frame.origin.y as f32,
            width: frame.size.width as f32,
            height: frame.size.height as f32,
        })
    }
    fn absolute_frame(&self, node: &dyn Any) -> Option<ViewportRect> {
        // Same conversion as `rect_of_node`, but returns None when
        // not yet in a window instead of the zero-rect sentinel.
        let ios_node = node.downcast_ref::<IosNode>()?;
        let view = ios_node.as_view();
        let bounds: CGRect = unsafe { msg_send![view, bounds] };
        let window: Option<Retained<UIView>> = unsafe {
            let w: *mut UIView = msg_send![view, window];
            if w.is_null() { None } else { Retained::retain(w) }
        };
        let window = window?;
        let frame_in_window: CGRect = unsafe {
            msg_send![view, convertRect: bounds, toView: &*window]
        };
        Some(ViewportRect {
            x: frame_in_window.origin.x as f32,
            y: frame_in_window.origin.y as f32,
            width: frame_in_window.size.width as f32,
            height: frame_in_window.size.height as f32,
        })
    }

    /// Route `AnimatedValue::bind` writes through the existing
    /// `backend_ios_mobile::set_animated_f32` free function so the
    /// framework's animation-binding helper doesn't have to know
    /// about `IosNode`. Downcasts to `IosNode` (the handle's
    /// concrete node type); silently no-ops if the cast fails
    /// (which would mean the framework handed us a node from a
    /// different backend).
    fn set_animated_f32(
        &self,
        node: &dyn Any,
        prop: runtime_core::animation::AnimProp,
        value: f32,
    ) {
        if let Some(n) = node.downcast_ref::<IosNode>() {
            crate::imp::set_animated_f32(n, prop, value);
        }
    }

    /// Color-family analog of [`Self::set_animated_f32`].
    fn set_animated_color(
        &self,
        node: &dyn Any,
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        if let Some(n) = node.downcast_ref::<IosNode>() {
            crate::imp::set_animated_color(n, prop, value);
        }
    }
}
pub(crate) static IOS_VIEW_OPS: IosViewOps = IosViewOps;

// `IosTextOps` provides the same animated-color dispatch as
// `IosViewOps` but on a text-bearing widget — `set_animated_color`
// routes to `set_animated_color` on the backend, which (in turn)
// dispatches to `UILabel.textColor` for label nodes.
pub(crate) struct IosTextOps;
impl runtime_core::TextOps for IosTextOps {
    fn set_animated_color(
        &self,
        node: &dyn Any,
        prop: runtime_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        if let Some(n) = node.downcast_ref::<IosNode>() {
            crate::imp::set_animated_color(n, prop, value);
        }
    }
}
pub(crate) static IOS_TEXT_OPS: IosTextOps = IosTextOps;

/// UITextField-backed implementation of the framework's
/// `TextInputOps`. Handles the imperative surface exposed via
/// `Ref<TextInputHandle>`: `focus`, `blur`, `select_all`, and
/// `insert_text`. The handle's `node: Rc<dyn Any>` is the
/// `Retained<UITextField>` we boxed in `make_text_input_handle`.
pub(crate) struct IosTextInputOps;
impl TextInputOps for IosTextInputOps {
    fn focus(&self, node: &dyn Any) {
        if let Some(field) = node.downcast_ref::<Retained<UITextField>>() {
            let _: bool = unsafe { msg_send![&**field, becomeFirstResponder] };
        }
    }
    fn blur(&self, node: &dyn Any) {
        if let Some(field) = node.downcast_ref::<Retained<UITextField>>() {
            let _: bool = unsafe { msg_send![&**field, resignFirstResponder] };
        }
    }
    fn select_all(&self, node: &dyn Any) {
        if let Some(field) = node.downcast_ref::<Retained<UITextField>>() {
            // `selectAll:` is the responder-chain action UIKit uses
            // for the system "Select All" menu. Passing nil as the
            // sender mirrors how a user-initiated selection would be
            // dispatched.
            let nil: *mut NSObject = std::ptr::null_mut();
            let _: () = unsafe { msg_send![&**field, selectAll: nil] };
        }
    }
    fn insert_text(&self, node: &dyn Any, text: &str) {
        // UITextField conforms to the `UIKeyInput` protocol;
        // `insertText:` replaces the current selection (or inserts at
        // the caret if none) with the given NSString and advances the
        // caret. This is what UIKit calls internally when the user
        // types — so going through it fires `editingChanged` too,
        // which means the controlling `Signal` updates without us
        // touching it manually.
        if let Some(field) = node.downcast_ref::<Retained<UITextField>>() {
            let ns = NSString::from_str(text);
            let _: () = unsafe { msg_send![&**field, insertText: &*ns] };
        }
    }
}
pub(crate) static IOS_TEXT_INPUT_OPS: IosTextInputOps = IosTextInputOps;

/// UITextView-backed `TextAreaOps`. Mirror of [`IosTextInputOps`];
/// the only differences are the underlying UIKit widget and the
/// `selectAll:` dispatch path (UITextView accepts the same selector
/// via its responder chain).
pub(crate) struct IosTextAreaOps;
impl TextAreaOps for IosTextAreaOps {
    fn focus(&self, node: &dyn Any) {
        if let Some(view) = node.downcast_ref::<Retained<UITextView>>() {
            let _: bool = unsafe { msg_send![&**view, becomeFirstResponder] };
        }
    }
    fn blur(&self, node: &dyn Any) {
        if let Some(view) = node.downcast_ref::<Retained<UITextView>>() {
            let _: bool = unsafe { msg_send![&**view, resignFirstResponder] };
        }
    }
    fn select_all(&self, node: &dyn Any) {
        if let Some(view) = node.downcast_ref::<Retained<UITextView>>() {
            let nil: *mut NSObject = std::ptr::null_mut();
            let _: () = unsafe { msg_send![&**view, selectAll: nil] };
        }
    }
    fn insert_text(&self, node: &dyn Any, text: &str) {
        if let Some(view) = node.downcast_ref::<Retained<UITextView>>() {
            let ns = NSString::from_str(text);
            let _: () = unsafe { msg_send![&**view, insertText: &*ns] };
        }
    }
}
pub(crate) static IOS_TEXT_AREA_OPS: IosTextAreaOps = IosTextAreaOps;
