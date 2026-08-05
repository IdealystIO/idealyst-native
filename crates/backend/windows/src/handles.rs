//! Node handles — the bridge from `Ref<ViewHandle>` / `Ref<TextHandle>`
//! back to the backend.
//!
//! `AnimatedValue::bind(ref, prop)` fills a `Ref` with a handle built by
//! [`Backend::make_view_handle`](runtime_shared::Backend::make_view_handle)
//! / `make_text_handle`, then drives per-frame writes through that
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
use runtime_shared::{Backend, TextHandle, TextOps, ViewHandle, ViewOps};

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
