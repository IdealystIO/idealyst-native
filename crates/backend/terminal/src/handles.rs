//! Backend-provided `*Ops` impls so `AnimatedValue::bind(...)` writes
//! actually reach our [`TerminalBackend::set_animated_f32`] /
//! [`set_animated_color`] paths.
//!
//! Without these, the framework's `Ref<ViewHandle>::with(|h| ...)`
//! call ends up on `NoopViewOps`, every animation tick silently
//! discards its value, and the screen never updates. Mirrors the
//! macOS / iOS handle-ops bridge.

use std::any::Any;
use std::rc::Rc;

use framework_core::animation::AnimProp;
use framework_core::primitives::portal::ViewportRect;
use framework_core::{TextHandle, TextOps, ViewHandle, ViewOps};

use crate::node::TermNode;
use crate::GLOBAL_BACKEND;

pub(crate) struct TermViewOps;

impl ViewOps for TermViewOps {
    fn rect(&self, node: &dyn Any) -> ViewportRect {
        frame_of(node).unwrap_or(ViewportRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        })
    }

    fn frame(&self, node: &dyn Any) -> Option<ViewportRect> {
        frame_of(node)
    }

    fn absolute_frame(&self, node: &dyn Any) -> Option<ViewportRect> {
        // Terminal coordinates are already viewport-absolute since
        // we don't have a window-vs-screen distinction. Same value
        // as `frame`.
        frame_of(node)
    }

    fn set_animated_f32(&self, node: &dyn Any, prop: AnimProp, value: f32) {
        let Some(n) = node.downcast_ref::<TermNode>() else { return };
        with_backend(|b| {
            use framework_core::Backend;
            b.set_animated_f32(n, prop, value);
        });
    }

    fn set_animated_color(&self, node: &dyn Any, prop: AnimProp, value: [f32; 4]) {
        let Some(n) = node.downcast_ref::<TermNode>() else { return };
        with_backend(|b| {
            use framework_core::Backend;
            b.set_animated_color(n, prop, value);
        });
    }
}

pub(crate) struct TermTextOps;

impl TextOps for TermTextOps {
    fn set_animated_color(&self, node: &dyn Any, prop: AnimProp, value: [f32; 4]) {
        let Some(n) = node.downcast_ref::<TermNode>() else { return };
        with_backend(|b| {
            use framework_core::Backend;
            b.set_animated_color(n, prop, value);
        });
    }
}

pub(crate) static TERM_VIEW_OPS: TermViewOps = TermViewOps;
pub(crate) static TERM_TEXT_OPS: TermTextOps = TermTextOps;

pub(crate) fn make_view_handle(node: &TermNode) -> ViewHandle {
    ViewHandle::new(Rc::new(*node) as Rc<dyn Any>, &TERM_VIEW_OPS)
}

pub(crate) fn make_text_handle(node: &TermNode) -> TextHandle {
    TextHandle::new(Rc::new(*node) as Rc<dyn Any>, &TERM_TEXT_OPS)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn frame_of(node: &dyn Any) -> Option<ViewportRect> {
    let n = node.downcast_ref::<TermNode>()?;
    with_backend(|b| {
        let data = b.nodes.get(&n.id)?;
        let f = b.layout.frame_of(data.layout);
        Some(ViewportRect {
            x: f.x,
            y: f.y,
            width: f.width,
            height: f.height,
        })
    })
    .flatten()
}

fn with_backend<R>(f: impl FnOnce(&mut crate::TerminalBackend) -> R) -> Option<R> {
    let weak = GLOBAL_BACKEND.with(|s| s.borrow().clone())?;
    let rc = weak.upgrade()?;
    let mut b = rc.try_borrow_mut().ok()?;
    Some(f(&mut b))
}
