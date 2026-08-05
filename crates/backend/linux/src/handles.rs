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
//! the [`LinuxBackend`].
//!
//! The handle's state carries a `Weak` reference to the backend (which
//! lives in an `Rc<RefCell<LinuxBackend>>` owned by the host) plus the
//! node. Ops upgrade the weak and `try_borrow_mut` — `try_` because a
//! handle write could, in principle, land while the mount walk still
//! holds the backend borrow; skipping then is safe since the animation
//! clock re-applies within a frame.

use gtk4::prelude::*;

use std::any::Any;
use std::cell::RefCell;
use std::rc::Weak;

use runtime_shared::animation::AnimProp;
use runtime_shared::primitives::portal::ViewportRect;
use runtime_shared::{Backend, TextHandle, TextOps, ViewHandle, ViewOps};

use crate::{LinuxBackend, LinuxNode};

/// State stored inside a `ViewHandle` / `TextHandle`: how to reach the
/// backend, and which node this handle targets.
pub(crate) struct HandleState {
    pub backend: Weak<RefCell<LinuxBackend>>,
    pub node: LinuxNode,
}

impl HandleState {
    fn with_backend_mut(&self, f: impl FnOnce(&mut LinuxBackend, &LinuxNode)) {
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

struct LinuxViewOps;
static LINUX_VIEW_OPS: LinuxViewOps = LinuxViewOps;

impl ViewOps for LinuxViewOps {
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
        b.node_absolute_frame(state.node.id)
            .map(|(x, y, w, h)| ViewportRect {
                x,
                y,
                width: w,
                height: h,
            })
    }
}

pub(crate) fn make_view_handle(backend: &LinuxBackend, node: &LinuxNode) -> ViewHandle {
    ViewHandle::new(
        std::rc::Rc::new(HandleState {
            backend: backend.self_ref(),
            node: node.clone(),
        }) as std::rc::Rc<dyn Any>,
        &LINUX_VIEW_OPS,
    )
}

// =========================================================================
// Text handle
// =========================================================================

struct LinuxTextOps;
static LINUX_TEXT_OPS: LinuxTextOps = LinuxTextOps;

impl TextOps for LinuxTextOps {
    fn set_animated_color(&self, node: &dyn Any, prop: AnimProp, value: [f32; 4]) {
        if let Some(state) = node.downcast_ref::<HandleState>() {
            state.with_backend_mut(|b, n| b.set_animated_color(n, prop, value));
        }
    }
}

pub(crate) fn make_text_handle(backend: &LinuxBackend, node: &LinuxNode) -> TextHandle {
    TextHandle::new(
        std::rc::Rc::new(HandleState {
            backend: backend.self_ref(),
            node: node.clone(),
        }) as std::rc::Rc<dyn Any>,
        &LINUX_TEXT_OPS,
    )
}

// =========================================================================
// Scroll-view handle
// =========================================================================

/// Imperative ops for a `scroll_view` ref. Without these the framework
/// installs `NoopScrollViewOps` and every `scroll_to` silently does
/// nothing — which is why clicking a table-of-contents entry didn't jump
/// to its section.
struct LinuxScrollViewOps;

impl runtime_shared::primitives::scroll_view::ScrollViewOps for LinuxScrollViewOps {
    fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {
        let Some(state) = node.downcast_ref::<HandleState>() else {
            return;
        };
        let Some(sw) = state.node.widget().downcast_ref::<gtk4::ScrolledWindow>() else {
            return;
        };
        // Clamp into the adjustment's scrollable range. GTK clamps on its
        // own, but doing it here keeps the value we set and the value the
        // adjustment reports in agreement for any follow-up read.
        let hadj = sw.hadjustment();
        let vadj = sw.vadjustment();
        hadj.set_value(
            (x as f64).clamp(hadj.lower(), (hadj.upper() - hadj.page_size()).max(hadj.lower())),
        );
        vadj.set_value(
            (y as f64).clamp(vadj.lower(), (vadj.upper() - vadj.page_size()).max(vadj.lower())),
        );
    }
}

static LINUX_SCROLL_VIEW_OPS: LinuxScrollViewOps = LinuxScrollViewOps;

pub(crate) fn make_scroll_view_handle(
    backend: &LinuxBackend,
    node: &LinuxNode,
) -> runtime_shared::primitives::scroll_view::ScrollViewHandle {
    runtime_shared::primitives::scroll_view::ScrollViewHandle::new(
        std::rc::Rc::new(HandleState {
            backend: backend.self_ref(),
            node: node.clone(),
        }) as std::rc::Rc<dyn Any>,
        &LINUX_SCROLL_VIEW_OPS,
    )
}

#[cfg(test)]
pub(crate) fn scroll_view_ops_for_test() -> &'static dyn runtime_shared::primitives::scroll_view::ScrollViewOps
{
    &LINUX_SCROLL_VIEW_OPS
}

#[cfg(test)]
pub(crate) fn handle_state_for_test(backend: &LinuxBackend, node: &LinuxNode) -> HandleState {
    HandleState {
        backend: backend.self_ref(),
        node: node.clone(),
    }
}
