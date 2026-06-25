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
use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::primitives::portal::ViewportRect;
use runtime_core::{LayoutSubscription, ViewOps, ViewHandle, TextHandle};
use objc2::msg_send;
use objc2_app_kit::NSView;
use objc2_foundation::CGRect;

use crate::imp::MacosNode;

thread_local! {
    /// Per-view `on_layout` callbacks, keyed by the NSView pointer.
    /// Fired from `compute_and_apply_layout` after each view's frame is
    /// resolved, which is how a `.container()` view's inline-size signal
    /// gets fed on macOS (the AppKit analog of the web `ResizeObserver`).
    /// macOS is single-threaded (main-thread `mtm`), so a thread-local
    /// registry is safe.
    static LAYOUT_SUBS: RefCell<Vec<(usize, Rc<dyn Fn(f32, f32)>)>> =
        const { RefCell::new(Vec::new()) };
}

/// Stable pointer key for an `NSView`, shared by `subscribe_layout`
/// (which stores `&NSView`) and the layout loop (which holds a
/// `Retained<NSView>`). Both must derive the key the same way.
pub(crate) fn view_key(view: &NSView) -> usize {
    view as *const NSView as usize
}

/// Fire every `on_layout` callback registered for `view_key` with the
/// view's resolved inline-size (`w`) and block-size (`h`). Called once
/// per view per layout pass; the callbacks themselves change-guard, so
/// re-firing at an unchanged size is a no-op.
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

// =========================================================================
// View ops
// =========================================================================

pub(crate) struct MacosViewOps;

impl ViewOps for MacosViewOps {
    fn subscribe_layout(
        &self,
        node: &dyn Any,
        callback: Box<dyn Fn(f32, f32)>,
    ) -> LayoutSubscription {
        let Some(macos_node) = node.downcast_ref::<MacosNode>() else {
            return LayoutSubscription::noop();
        };
        let key = view_key(macos_node.as_view());
        let cb: Rc<dyn Fn(f32, f32)> = Rc::from(callback);
        let cb_id = Rc::as_ptr(&cb) as *const () as usize;
        LAYOUT_SUBS.with(|m| m.borrow_mut().push((key, cb)));
        // RAII: drop removes exactly this callback (matched by view key +
        // Rc identity) so a container unmount tears its subscription down.
        LayoutSubscription::new(move || {
            LAYOUT_SUBS.with(|m| {
                m.borrow_mut()
                    .retain(|(k, c)| !(*k == key && Rc::as_ptr(c) as *const () as usize == cb_id))
            });
        })
    }

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
        // Convert into the window's `contentView`, NOT `nil`. Passing `nil`
        // yields AppKit's window *base* coordinates, which are BOTTOM-LEFT
        // (y grows up). The contentView is the `isFlipped` full-window host, so
        // its space is TOP-LEFT window coordinates (y grows down) — matching the
        // touch `window_position` path (see `view.rs`, which converts a point
        // into the same contentView) and the documented top-left
        // `ViewportRect` / `TouchPoint` convention. Without this, this rect's Y
        // is inverted relative to touch coordinates, so any window-space
        // hit-testing (drag-and-drop collision, overlay anchoring) silently
        // fails on macOS.
        let content: *mut NSView = unsafe { msg_send![window, contentView] };
        let to_view: *mut NSView = if content.is_null() {
            std::ptr::null_mut()
        } else {
            content
        };
        let frame_in_window: CGRect = unsafe {
            msg_send![view, convertRect: bounds, toView: to_view]
        };
        // `convertRect:` is frame-based and ignores the CALayer transform — so a
        // view positioned by an animated `TranslateX/Y` reports its untransformed
        // frame, not where it visually sits. Add the translate so the rect is the
        // VISUAL window rect, matching the touch `window_position` the SDK
        // hit-tests it against (without this, drag-and-drop drops land offset by
        // the transform — more the further the target sits from the origin).
        let (tx, ty) = super::animated::view_layer_translate(view);
        Some(ViewportRect {
            x: frame_in_window.origin.x as f32 + tx as f32,
            y: frame_in_window.origin.y as f32 + ty as f32,
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
        prop: runtime_core::animation::AnimProp,
        value: f32,
    ) {
        if let Some(n) = node.downcast_ref::<MacosNode>() {
            crate::imp::set_animated_f32(n, prop, value);
        }
    }

    fn set_animated_color(
        &self,
        node: &dyn Any,
        prop: runtime_core::animation::AnimProp,
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

impl runtime_core::TextOps for MacosTextOps {
    fn set_animated_color(
        &self,
        node: &dyn Any,
        prop: runtime_core::animation::AnimProp,
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

// =========================================================================
// Text input / text area ops — programmatic focus / blur / select / insert.
// =========================================================================
//
// macOS has no built-in bridge for the SDK's `TextInputHandle::focus()` etc.
// (the framework falls back to `NoopTextInputOps`, which silently does
// nothing — so a controlled editor can't put the caret in the field). These
// impls dispatch through the AppKit responder chain: `focus()` makes the
// widget the window's first responder (which is what shows the blinking
// insertion point and starts key delivery). Mirrors the iOS handles.

use objc2_foundation::NSString;
use runtime_core::primitives::text_area::{TextAreaHandle, TextAreaOps};
use runtime_core::primitives::text_input::{TextInputHandle, TextInputOps};

/// Make `node`'s view the window's first responder — shows the caret and
/// routes keystrokes to it. No-op until the view is in a window.
fn make_first_responder(node: &dyn Any) {
    let Some(n) = node.downcast_ref::<MacosNode>() else { return };
    let view = n.as_view();
    unsafe {
        let window: *mut objc2::runtime::AnyObject = msg_send![view, window];
        if !window.is_null() {
            let _: bool = msg_send![window, makeFirstResponder: view];
        }
    }
}

/// Resign first responder (clears the caret), via the window.
fn resign_first_responder(node: &dyn Any) {
    let Some(n) = node.downcast_ref::<MacosNode>() else { return };
    let view = n.as_view();
    unsafe {
        let window: *mut objc2::runtime::AnyObject = msg_send![view, window];
        if !window.is_null() {
            let nil: *mut objc2::runtime::AnyObject = std::ptr::null_mut();
            let _: bool = msg_send![window, makeFirstResponder: nil];
        }
    }
}

/// Select the field's whole contents (`-[NSText selectAll:]`).
fn select_all_text(node: &dyn Any) {
    let Some(n) = node.downcast_ref::<MacosNode>() else { return };
    let view = n.as_view();
    let nil: *mut objc2::runtime::AnyObject = std::ptr::null_mut();
    unsafe {
        let _: () = msg_send![view, selectAll: nil];
    }
}

/// Insert `text` at the caret (best-effort; the controlling `Signal` stays the
/// source of truth via the widget's normal change notification).
fn insert_text_into(node: &dyn Any, text: &str) {
    let Some(n) = node.downcast_ref::<MacosNode>() else { return };
    let view = n.as_view();
    let s = NSString::from_str(text);
    unsafe {
        let _: () = msg_send![view, insertText: &*s];
    }
}

pub(crate) struct MacosTextInputOps;
impl TextInputOps for MacosTextInputOps {
    fn focus(&self, node: &dyn Any) {
        make_first_responder(node);
    }
    fn blur(&self, node: &dyn Any) {
        resign_first_responder(node);
    }
    fn select_all(&self, node: &dyn Any) {
        select_all_text(node);
    }
    fn insert_text(&self, node: &dyn Any, text: &str) {
        insert_text_into(node, text);
    }
}
pub(crate) static MACOS_TEXT_INPUT_OPS: MacosTextInputOps = MacosTextInputOps;

pub(crate) struct MacosTextAreaOps;
impl TextAreaOps for MacosTextAreaOps {
    fn focus(&self, node: &dyn Any) {
        make_first_responder(node);
    }
    fn blur(&self, node: &dyn Any) {
        resign_first_responder(node);
    }
    fn select_all(&self, node: &dyn Any) {
        select_all_text(node);
    }
    fn insert_text(&self, node: &dyn Any, text: &str) {
        insert_text_into(node, text);
    }
}
pub(crate) static MACOS_TEXT_AREA_OPS: MacosTextAreaOps = MacosTextAreaOps;

/// Build a focus-capable handle for a `text_input` (`NSTextField`) node.
pub(crate) fn make_text_input_handle(node: &MacosNode) -> TextInputHandle {
    TextInputHandle::new(Rc::new(node.clone()), &MACOS_TEXT_INPUT_OPS)
}

/// Build a focus-capable handle for a `text_area` (`NSTextView`) node.
pub(crate) fn make_text_area_handle(node: &MacosNode) -> TextAreaHandle {
    TextAreaHandle::new(Rc::new(node.clone()), &MACOS_TEXT_AREA_OPS)
}
