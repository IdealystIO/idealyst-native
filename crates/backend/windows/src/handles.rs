//! Node handles — the bridge from `Ref<ViewHandle>` / `Ref<TextHandle>`
//! back to the backend.
//!
//! `AnimatedValue::bind(ref, prop)` fills a `Ref` with a handle built by
//! `caps::ViewOps::make_view_handle` / `caps::TextOps::make_text_handle`,
//! then drives per-frame writes through that
//! handle's [`ViewOps`] / [`TextOps`]. The default trait impls return a
//! **no-op** handle, so a backend that doesn't build real ones silently
//! drops every animation (the value ticks, nothing paints). This module
//! supplies handles that route `set_animated_*` and `frame()` back into
//! the [`WindowsBackend`]. Direct port of the Linux backend's
//! `handles.rs` — the plumbing is platform-independent.
//!
//! The handle's state carries a `Weak` reference to the backend (which
//! lives in an `Rc<RefCell<WindowsBackend>>` owned by the host) plus the
//! node. Ops upgrade the weak and `try_borrow_mut` — `try_` because a
//! handle write could, in principle, land while the mount walk still
//! holds the backend borrow; skipping then is safe since the animation
//! clock re-applies within a frame.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Weak;

use runtime_shared::animation::AnimProp;
use runtime_shared::primitives::portal::ViewportRect;
use runtime_shared::{LayoutSubscription, TextHandle, TextOps, ViewHandle, ViewOps};

use crate::{WindowsBackend, WindowsNode};

/// State stored inside a `ViewHandle` / `TextHandle`: how to reach the
/// backend, and which node this handle targets.
pub(crate) struct HandleState {
    pub backend: Weak<RefCell<WindowsBackend>>,
    pub node: WindowsNode,
}

impl HandleState {
    fn with_backend_mut(&self, f: impl FnOnce(&mut WindowsBackend, &WindowsNode)) {
        if let Some(b) = self.backend.upgrade() {
            if let Ok(mut b) = b.try_borrow_mut() {
                f(&mut b, &self.node);
            }
        }
    }
}

// =========================================================================
// View handle
// =========================================================================

struct Win32ViewOps;
static WIN32_VIEW_OPS: Win32ViewOps = Win32ViewOps;

impl ViewOps for Win32ViewOps {
    fn set_animated_f32(&self, node: &dyn Any, prop: AnimProp, value: f32) {
        if let Some(state) = node.downcast_ref::<HandleState>() {
            state.with_backend_mut(|b, n| b.set_animated_f32(n, prop, value));
        }
    }

    fn set_animated_color(&self, node: &dyn Any, prop: AnimProp, value: [f32; 4]) {
        if let Some(state) = node.downcast_ref::<HandleState>() {
            state.with_backend_mut(|b, n| b.set_animated_color(n, prop, value));
        }
    }

    fn frame(&self, node: &dyn Any) -> Option<ViewportRect> {
        let state = node.downcast_ref::<HandleState>()?;
        let b = state.backend.upgrade()?;
        let b = b.try_borrow().ok()?;
        b.node_frame(state.node.id).map(|(x, y, w, h)| ViewportRect {
            x,
            y,
            width: w,
            height: h,
        })
    }

    fn absolute_frame(&self, node: &dyn Any) -> Option<ViewportRect> {
        let state = node.downcast_ref::<HandleState>()?;
        let b = state.backend.upgrade()?;
        let b = b.try_borrow().ok()?;
        b.node_abs_frame(state.node.id).map(|(x, y, w, h)| ViewportRect {
            x,
            y,
            width: w,
            height: h,
        })
    }

    /// Window-relative, like `absolute_frame` — overlays anchor to it and
    /// charts seed their plot size from it. The zero rect is the documented
    /// "not laid out yet" sentinel.
    fn rect(&self, node: &dyn Any) -> ViewportRect {
        self.absolute_frame(node).unwrap_or(ViewportRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 })
    }

    /// See `crate::layout_subs`. The first size is delivered on the next
    /// scheduler turn (not from inside this call, which may run under a
    /// mount walk that holds the backend).
    fn subscribe_layout(&self, node: &dyn Any, callback: Box<dyn Fn(f32, f32)>) -> LayoutSubscription {
        let Some(state) = node.downcast_ref::<HandleState>() else {
            return LayoutSubscription::noop();
        };
        let sub = crate::layout_subs::subscribe(state.node.id, callback);
        let backend = state.backend.clone();
        runtime_shared::scheduling::after_ms_detached(0, move || {
            let Some(b) = backend.upgrade() else { return };
            let Ok(b) = b.try_borrow() else { return };
            let due = b.layout_subs_due();
            drop(b);
            crate::layout_subs::fire(due);
        });
        sub
    }
}

pub(crate) fn make_view_handle(backend: &WindowsBackend, node: &WindowsNode) -> ViewHandle {
    ViewHandle::new(
        std::rc::Rc::new(HandleState {
            backend: backend.self_ref(),
            node: node.clone(),
        }) as std::rc::Rc<dyn Any>,
        &WIN32_VIEW_OPS,
    )
}

// =========================================================================
// Text handle
// =========================================================================

struct Win32TextOps;
static WIN32_TEXT_OPS: Win32TextOps = Win32TextOps;

impl TextOps for Win32TextOps {
    fn set_animated_color(&self, node: &dyn Any, prop: AnimProp, value: [f32; 4]) {
        if let Some(state) = node.downcast_ref::<HandleState>() {
            state.with_backend_mut(|b, n| b.set_animated_color(n, prop, value));
        }
    }
}

pub(crate) fn make_text_handle(backend: &WindowsBackend, node: &WindowsNode) -> TextHandle {
    TextHandle::new(
        std::rc::Rc::new(HandleState {
            backend: backend.self_ref(),
            node: node.clone(),
        }) as std::rc::Rc<dyn Any>,
        &WIN32_TEXT_OPS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NodeKind, NodeMeta, ViewVisual};
    use std::cell::RefCell;
    use std::rc::Rc;
    use windows::Win32::Foundation::HWND;

    /// A component that sizes itself from its laid-out box (the charts SDK's
    /// plot) read `rect()` and subscribed `on_layout` through its view handle;
    /// on Windows `rect()` was the zero rect and the subscription a no-op, so
    /// the plot stayed 0×0 and every chart drew nothing.
    #[test]
    fn regression_view_handle_reports_rect_and_layout_size() {
        let backend = Rc::new(RefCell::new(WindowsBackend::new(HWND(std::ptr::null_mut()))));
        backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
        let node = {
            let mut b = backend.borrow_mut();
            let mut m = NodeMeta::new(NodeKind::View(ViewVisual::default()));
            m.frame = (4.0, 6.0, 226.0, 40.0);
            m.abs = (24.0, 704.0);
            b.nodes.insert(9, m);
            b.root_id = Some(9);
            WindowsNode { id: 9, hwnd: HWND(std::ptr::null_mut()) }
        };
        let handle = make_view_handle(&backend.borrow(), &node);
        let r = runtime_shared::primitives::portal::AnchorableHandle::rect(&handle);
        assert_eq!((r.x, r.y, r.width, r.height), (24.0, 704.0, 226.0, 40.0));

        let seen: Rc<RefCell<Vec<(f32, f32)>>> = Default::default();
        let s = seen.clone();
        // No scheduler is installed in tests, so the first delivery runs
        // synchronously.
        let _sub = handle.on_layout(move |w, h| s.borrow_mut().push((w, h)));
        assert_eq!(*seen.borrow(), [(226.0, 40.0)], "the current size on subscribe");

        backend.borrow_mut().nodes.get_mut(&9).unwrap().frame = (4.0, 6.0, 300.0, 40.0);
        let due = backend.borrow().layout_subs_due();
        crate::layout_subs::fire(due);
        assert_eq!(*seen.borrow(), [(226.0, 40.0), (300.0, 40.0)], "and each new size after a pass");
    }
}
