//! iOS-specific `*Ops` impls — currently scoped to providing
//! viewport-relative rects so views can serve as overlay anchors
//! (`Popover`, `Select`, etc.).
//!
//! Each Ops impl is a zero-sized type with a `'static` instance. The
//! backend's `make_*_handle` methods reference the appropriate static
//! when constructing the handle.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2_foundation::{CGRect, NSObject};
use objc2_ui_kit::UIView;
use std::any::Any;

use framework_core::primitives::overlay::ViewportRect;
use framework_core::{ButtonOps, PressableOps, ViewOps};

use crate::imp::IosNode;

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
    fn rect(&self, node: &dyn Any) -> ViewportRect {
        rect_of_node(node)
    }
}
pub(crate) static IOS_VIEW_OPS: IosViewOps = IosViewOps;
