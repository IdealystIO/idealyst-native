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

// =========================================================================
// Scroll view + virtualizer handles
// =========================================================================

struct Win32ScrollOps;
static WIN32_SCROLL_OPS: Win32ScrollOps = Win32ScrollOps;

impl runtime_shared::primitives::scroll_view::ScrollViewOps for Win32ScrollOps {
    fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {
        if let Some(state) = node.downcast_ref::<HandleState>() {
            state.with_backend_mut(|b, n| {
                b.set_node_scroll(n, x, y);
            });
        }
    }
}

pub(crate) fn make_scroll_view_handle(
    backend: &WindowsBackend,
    node: &WindowsNode,
) -> runtime_shared::primitives::scroll_view::ScrollViewHandle {
    runtime_shared::primitives::scroll_view::ScrollViewHandle::new(
        std::rc::Rc::new(HandleState { backend: backend.self_ref(), node: node.clone() }) as std::rc::Rc<dyn Any>,
        &WIN32_SCROLL_OPS,
    )
}

struct Win32VirtualizerOps;
static WIN32_VIRTUALIZER_OPS: Win32VirtualizerOps = Win32VirtualizerOps;

impl runtime_shared::primitives::virtualizer::VirtualizerOps for Win32VirtualizerOps {
    /// Bring item `index`'s leading edge to the top (left) of the list.
    fn scroll_to_index(&self, node: &dyn Any, index: usize) {
        if let Some(state) = node.downcast_ref::<HandleState>() {
            state.with_backend_mut(|b, n| {
                let Some(st) = b.virtualizers.get(&n.id) else { return };
                let Some(&offset) = st.offsets.get(index) else { return };
                let horizontal = st.horizontal;
                let (x, y) = if horizontal { (offset, 0.0) } else { (0.0, offset) };
                b.set_node_scroll(n, x, y);
            });
        }
    }

    fn scroll_offset(&self, node: &dyn Any) -> (f32, f32) {
        let Some(state) = node.downcast_ref::<HandleState>() else { return (0.0, 0.0) };
        let Some(b) = state.backend.upgrade() else { return (0.0, 0.0) };
        let Ok(b) = b.try_borrow() else { return (0.0, 0.0) };
        b.node_scroll(&state.node)
    }

    fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {
        if let Some(state) = node.downcast_ref::<HandleState>() {
            state.with_backend_mut(|b, n| b.set_node_scroll(n, x, y));
        }
    }
}

pub(crate) fn make_virtualizer_handle(
    backend: &WindowsBackend,
    node: &WindowsNode,
) -> runtime_shared::primitives::virtualizer::VirtualizerHandle {
    runtime_shared::primitives::virtualizer::VirtualizerHandle::new(
        std::rc::Rc::new(HandleState { backend: backend.self_ref(), node: node.clone() }) as std::rc::Rc<dyn Any>,
        &WIN32_VIRTUALIZER_OPS,
    )
}

// =========================================================================
// Text input / text area handles
// =========================================================================
//
// A field is a child `EDIT` window created already attached to the host,
// so the attach-safe focus contract is met by focusing at once. The ops act
// on the HWND alone and never borrow the backend: `EM_REPLACESEL` sends
// `EN_CHANGE` synchronously, and its `on_change` must find the backend
// free.

struct Win32FieldOps;
static WIN32_FIELD_OPS: Win32FieldOps = Win32FieldOps;

/// The field's HWND, carried by its handle.
struct FieldHandle(windows::Win32::Foundation::HWND);

fn field_hwnd(node: &dyn Any) -> Option<windows::Win32::Foundation::HWND> {
    node.downcast_ref::<FieldHandle>().map(|h| h.0).filter(|h| !h.is_invalid())
}

fn field_focus(node: &dyn Any) {
    if let Some(h) = field_hwnd(node) {
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(h));
        }
    }
}

/// Hand focus back to the host window, if this field holds it.
fn field_blur(node: &dyn Any) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::GetParent;
    if let Some(h) = field_hwnd(node) {
        unsafe {
            if GetFocus() == h {
                if let Ok(parent) = GetParent(h) {
                    let _ = SetFocus(Some(parent));
                }
            }
        }
    }
}

fn field_select_all(node: &dyn Any) {
    if let Some(h) = field_hwnd(node) {
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                h,
                crate::EM_SETSEL,
                Some(windows::Win32::Foundation::WPARAM(0)),
                Some(windows::Win32::Foundation::LPARAM(-1)),
            );
        }
    }
}

/// Replace the selection with `text` (caret after it); the control's
/// `EN_CHANGE` reports the new value through `on_change`.
fn field_insert_text(node: &dyn Any, text: &str) {
    if let Some(h) = field_hwnd(node) {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                h,
                crate::EM_REPLACESEL,
                Some(windows::Win32::Foundation::WPARAM(1)),
                Some(windows::Win32::Foundation::LPARAM(wide.as_ptr() as isize)),
            );
        }
    }
}

impl runtime_shared::primitives::text_input::TextInputOps for Win32FieldOps {
    fn focus(&self, node: &dyn Any) {
        field_focus(node)
    }
    fn blur(&self, node: &dyn Any) {
        field_blur(node)
    }
    fn select_all(&self, node: &dyn Any) {
        field_select_all(node)
    }
    fn insert_text(&self, node: &dyn Any, text: &str) {
        field_insert_text(node, text)
    }
}

impl runtime_shared::primitives::text_area::TextAreaOps for Win32FieldOps {
    fn focus(&self, node: &dyn Any) {
        field_focus(node)
    }
    fn blur(&self, node: &dyn Any) {
        field_blur(node)
    }
    fn select_all(&self, node: &dyn Any) {
        field_select_all(node)
    }
    fn insert_text(&self, node: &dyn Any, text: &str) {
        field_insert_text(node, text)
    }
}

pub(crate) fn make_text_input_handle(node: &WindowsNode) -> runtime_shared::primitives::text_input::TextInputHandle {
    runtime_shared::primitives::text_input::TextInputHandle::new(
        std::rc::Rc::new(FieldHandle(node.hwnd)) as std::rc::Rc<dyn Any>,
        &WIN32_FIELD_OPS,
    )
}

pub(crate) fn make_text_area_handle(node: &WindowsNode) -> runtime_shared::primitives::text_area::TextAreaHandle {
    runtime_shared::primitives::text_area::TextAreaHandle::new(
        std::rc::Rc::new(FieldHandle(node.hwnd)) as std::rc::Rc<dyn Any>,
        &WIN32_FIELD_OPS,
    )
}
