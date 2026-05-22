//! Backend-provided `*Ops` impls for `ViewHandle` / `TextHandle`.
//!
//! Without these, the framework's `AnimatedValue::bind(ref)` writes
//! end up dispatching through `NoopViewOps` and silently no-op. The
//! framework can't know how to talk to our concrete `MacosNode`
//! type — only the backend can. These `*Ops` impls bridge the
//! gap by downcasting `node: &dyn Any` → `&MacosNode` and calling
//! into the per-prop writers in [`crate::imp::animated`].
//!
//! Mirrors the iOS handles in `backend-ios-mobile/src/imp/handles.rs`.

use std::any::Any;
use std::rc::Rc;

use framework_core::primitives::portal::ViewportRect;
use framework_core::{ViewOps, ViewHandle, TextHandle};
use objc2::msg_send;
use objc2_app_kit::NSView;
use objc2_foundation::CGRect;

use crate::imp::MacosNode;

// =========================================================================
// View ops
// =========================================================================

pub(crate) struct MacosViewOps;

impl ViewOps for MacosViewOps {
    fn rect(&self, node: &dyn Any) -> ViewportRect {
        rect_of_node(node).unwrap_or(ViewportRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        })
    }

    fn frame(&self, node: &dyn Any) -> Option<ViewportRect> {
        let macos_node = node.downcast_ref::<MacosNode>()?;
        let view = macos_node.as_view();
        let frame: CGRect = unsafe { msg_send![view, frame] };
        Some(ViewportRect {
            x: frame.origin.x as f32,
            y: frame.origin.y as f32,
            width: frame.size.width as f32,
            height: frame.size.height as f32,
        })
    }

    fn absolute_frame(&self, node: &dyn Any) -> Option<ViewportRect> {
        let macos_node = node.downcast_ref::<MacosNode>()?;
        let view = macos_node.as_view();
        let bounds: CGRect = unsafe { msg_send![view, bounds] };
        let window: *mut NSView = unsafe { msg_send![view, window] };
        if window.is_null() {
            return None;
        }
        // `-[NSView convertRect:toView:]` with the window's contentView
        // gives window coordinates. We use the window itself as the
        // reference (passing nil converts to window coords, same idea).
        let nil: *mut NSView = std::ptr::null_mut();
        let frame_in_window: CGRect = unsafe {
            msg_send![view, convertRect: bounds, toView: nil]
        };
        Some(ViewportRect {
            x: frame_in_window.origin.x as f32,
            y: frame_in_window.origin.y as f32,
            width: frame_in_window.size.width as f32,
            height: frame_in_window.size.height as f32,
        })
    }

    /// Route `AnimatedValue::bind` writes through the global
    /// `set_animated_f32` helper. Downcasts to `MacosNode`; silently
    /// no-ops if the cast fails (would mean a node from a different
    /// backend).
    fn set_animated_f32(
        &self,
        node: &dyn Any,
        prop: framework_core::animation::AnimProp,
        value: f32,
    ) {
        if let Some(n) = node.downcast_ref::<MacosNode>() {
            crate::imp::set_animated_f32(n, prop, value);
        }
    }

    fn set_animated_color(
        &self,
        node: &dyn Any,
        prop: framework_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        if let Some(n) = node.downcast_ref::<MacosNode>() {
            crate::imp::set_animated_color(n, prop, value);
        }
    }
}

pub(crate) static MACOS_VIEW_OPS: MacosViewOps = MacosViewOps;

// =========================================================================
// Text ops
// =========================================================================

pub(crate) struct MacosTextOps;

impl framework_core::TextOps for MacosTextOps {
    fn set_animated_color(
        &self,
        node: &dyn Any,
        prop: framework_core::animation::AnimProp,
        value: [f32; 4],
    ) {
        if let Some(n) = node.downcast_ref::<MacosNode>() {
            crate::imp::set_animated_color(n, prop, value);
        }
    }
}

pub(crate) static MACOS_TEXT_OPS: MacosTextOps = MacosTextOps;

// =========================================================================
// Constructors used by `Backend::make_*_handle`.
// =========================================================================

pub(crate) fn make_view_handle(node: &MacosNode) -> ViewHandle {
    ViewHandle::new(Rc::new(node.clone()) as Rc<dyn Any>, &MACOS_VIEW_OPS)
}

pub(crate) fn make_text_handle(node: &MacosNode) -> TextHandle {
    TextHandle::new(Rc::new(node.clone()) as Rc<dyn Any>, &MACOS_TEXT_OPS)
}

// =========================================================================
// Helpers
// =========================================================================

fn rect_of_node(node: &dyn Any) -> Option<ViewportRect> {
    let macos_node = node.downcast_ref::<MacosNode>()?;
    let view = macos_node.as_view();
    let frame: CGRect = unsafe { msg_send![view, frame] };
    Some(ViewportRect {
        x: frame.origin.x as f32,
        y: frame.origin.y as f32,
        width: frame.size.width as f32,
        height: frame.size.height as f32,
    })
}
