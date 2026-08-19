//! Node handles — the bridge from `Ref<ViewHandle>` / `Ref<TextHandle>`
//! back to the backend.
//!
//! `AnimatedValue::bind(ref, prop)` fills a `Ref` with a handle built by
//! `make_view_handle` / `make_text_handle` — mega-trait methods before
//! runtime-v2, inherent methods on [`LinuxBackend`] since — then drives
//! per-frame writes through that handle's [`ViewOps`] / [`TextOps`]. A
//! backend that doesn't build real handles silently
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
use runtime_shared::{TextHandle, TextOps, ViewHandle, ViewOps};

use crate::{LinuxBackend, LinuxNode};

/// State stored inside a `ViewHandle` / `TextHandle`: how to reach the
/// backend, and which node this handle targets.
pub(crate) struct HandleState {
    pub backend: Weak<RefCell<LinuxBackend>>,
    pub node: LinuxNode,
}

impl HandleState {
    /// The node's widget as a `GtkButton`, when it is one.
    fn widget_button(&self) -> Option<gtk4::Button> {
        self.node.widget.downcast_ref::<gtk4::Button>().cloned()
    }

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

    /// The rect an anchored overlay pins to. See [`anchor_rect`].
    fn rect(&self, node: &dyn Any) -> ViewportRect {
        anchor_rect(node)
    }
}

/// Viewport-relative rect for overlay anchoring, or the zero rect when the
/// node isn't laid out yet.
///
/// Every `*Ops::rect` on this backend routes here. The trait's default returns
/// the ZERO rect, which `resolve_anchored_placement` treats as a real 0x0
/// target at the window origin — so a backend that forgets to override it does
/// not fail loudly, it just pins every popover to the top-left corner. That is
/// exactly what happened here: `make_button_handle` was never implemented, so
/// `bind_to` handed the popover a no-op handle and the placement resolved
/// against `(0,0) 0x0`.
fn anchor_rect(node: &dyn Any) -> ViewportRect {
    let zero = ViewportRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 };
    let Some(state) = node.downcast_ref::<HandleState>() else {
        return zero;
    };
    let Some(b) = state.backend.upgrade() else {
        return zero;
    };
    let Ok(b) = b.try_borrow() else {
        return zero;
    };
    b.node_absolute_frame(state.node.id)
        .map(|(x, y, width, height)| ViewportRect { x, y, width, height })
        .unwrap_or(zero)
}

// =========================================================================
// Button / Pressable handles — the anchor targets for popovers
// =========================================================================

struct LinuxButtonOps;
static LINUX_BUTTON_OPS: LinuxButtonOps = LinuxButtonOps;

impl runtime_shared::ButtonOps for LinuxButtonOps {
    fn click(&self, node: &dyn Any) {
        if let Some(state) = node.downcast_ref::<HandleState>() {
            if let Some(b) = state.widget_button() {
                b.emit_clicked();
            }
        }
    }

    fn rect(&self, node: &dyn Any) -> ViewportRect {
        anchor_rect(node)
    }
}

struct LinuxPressableOps;
static LINUX_PRESSABLE_OPS: LinuxPressableOps = LinuxPressableOps;

impl runtime_shared::PressableOps for LinuxPressableOps {
    fn click(&self, node: &dyn Any) {
        // A pressable is an `IdealystView` with a click gesture; there is no
        // `emit_clicked` to fire, and the gesture's handler is owned by GTK.
        // Left unimplemented rather than faked — the robot's own click path
        // drives the author callback directly.
        let _ = node;
    }

    fn rect(&self, node: &dyn Any) -> ViewportRect {
        anchor_rect(node)
    }
}

pub(crate) fn make_button_handle(
    backend: &LinuxBackend,
    node: &LinuxNode,
) -> runtime_shared::ButtonHandle {
    runtime_shared::ButtonHandle::new(
        std::rc::Rc::new(HandleState {
            backend: backend.self_ref(),
            node: node.clone(),
        }) as std::rc::Rc<dyn Any>,
        &LINUX_BUTTON_OPS,
    )
}

pub(crate) fn make_pressable_handle(
    backend: &LinuxBackend,
    node: &LinuxNode,
) -> runtime_shared::PressableHandle {
    runtime_shared::PressableHandle::new(
        std::rc::Rc::new(HandleState {
            backend: backend.self_ref(),
            node: node.clone(),
        }) as std::rc::Rc<dyn Any>,
        &LINUX_PRESSABLE_OPS,
    )
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

// =========================================================================
// Text-input handle — `Ref<TextInputHandle>` and the robot's focus/blur verbs
//
// Was never implemented, so it fell to `NoopTextInputOps`: an author's
// `handle.focus()` / `blur()` / `select_all()` / `insert_text()` all did
// nothing on this backend, and the robot's `focus` / `blur` verbs silently
// no-opped — which also makes focus behaviour UNTESTABLE here (it is how a
// measurement of "does the focus ring apply" came back meaningless).
// =========================================================================

struct LinuxTextInputOps;
static LINUX_TEXT_INPUT_OPS: LinuxTextInputOps = LinuxTextInputOps;

impl LinuxTextInputOps {
    fn entry(node: &dyn Any) -> Option<gtk4::Entry> {
        node.downcast_ref::<HandleState>()?
            .node
            .widget
            .downcast_ref::<gtk4::Entry>()
            .cloned()
    }
}

impl runtime_shared::primitives::text_input::TextInputOps for LinuxTextInputOps {
    fn focus(&self, node: &dyn Any) {
        if let Some(e) = Self::entry(node) {
            e.grab_focus();
        }
    }

    fn blur(&self, node: &dyn Any) {
        if let Some(e) = Self::entry(node) {
            // GTK has no "unfocus this widget"; moving focus to the window
            // root is the toolkit's way of dropping it from a specific widget.
            if let Some(root) = e.root() {
                root.set_focus(None::<&gtk4::Widget>);
            }
        }
    }

    fn select_all(&self, node: &dyn Any) {
        if let Some(e) = Self::entry(node) {
            e.select_region(0, -1);
        }
    }

    fn insert_text(&self, node: &dyn Any, text: &str) {
        let Some(e) = Self::entry(node) else { return };
        // Replace the selection (or insert at the caret), then leave the caret
        // after the inserted text — and go through `Editable`, which fires the
        // entry's normal `changed` path so the controlling `Signal` observes
        // it, as the trait requires.
        if let Some((start, end)) = e.selection_bounds() {
            e.delete_text(start, end);
            e.set_position(start);
        }
        let mut pos = e.position();
        e.insert_text(text, &mut pos);
        e.set_position(pos);
    }
}

pub(crate) fn make_text_input_handle(
    backend: &LinuxBackend,
    node: &LinuxNode,
) -> runtime_shared::primitives::text_input::TextInputHandle {
    runtime_shared::primitives::text_input::TextInputHandle::new(
        std::rc::Rc::new(HandleState {
            backend: backend.self_ref(),
            node: node.clone(),
        }) as std::rc::Rc<dyn Any>,
        &LINUX_TEXT_INPUT_OPS,
    )
}
