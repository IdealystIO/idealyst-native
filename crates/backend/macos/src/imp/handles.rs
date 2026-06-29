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
        // Overlay anchoring contract: this MUST be the viewport/window-relative
        // rect, NOT the parent-relative `view.frame`. A trigger nested below the
        // window root has a frame in its parent's space, so feeding that to the
        // portal placement math anchored every tooltip/popover to the window's
        // top-left. Delegate to the same window-space conversion as
        // `absolute_frame`; the zero rect is the documented "centered fallback"
        // sentinel for the not-yet-mounted case.
        absolute_rect_of_node(node).unwrap_or(ViewportRect {
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
        absolute_rect_of_node(node)
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
// Button / Pressable ops — anchor measurement
// =========================================================================
//
// Without these, `make_button_handle` / `make_pressable_handle` fall back to
// the trait's `Noop*Ops`, whose `rect()` returns the ZERO rect. An overlay
// anchored to a Button or Pressable (every idea-ui `Popover`, whose trigger is
// a `Pressable`) then measured a zero-size trigger, so the anchor tracker
// early-returned every frame and the popover froze at its unmeasured fallback
// near the top-left. `rect()` here returns the same window-relative rect as
// `MacosViewOps` so a Button/Pressable anchor measures like a `view` one.
// Mirrors iOS's `IosButtonOps` / `IosPressableOps`.

pub(crate) struct MacosButtonOps;
impl runtime_core::ButtonOps for MacosButtonOps {
    fn click(&self, _node: &dyn Any) {
        // Programmatic click via the handle isn't wired on macOS (matches iOS):
        // the Robot drives clicks through the stored on_press action, and author
        // code rarely calls `handle.click()`. The previous behavior (NoopButtonOps)
        // was also a no-op, so this is no regression — only `rect()` gains a value.
    }
    fn rect(&self, node: &dyn Any) -> ViewportRect {
        absolute_rect_of_node(node).unwrap_or(ViewportRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 })
    }
}
pub(crate) static MACOS_BUTTON_OPS: MacosButtonOps = MacosButtonOps;

pub(crate) struct MacosPressableOps;
impl runtime_core::PressableOps for MacosPressableOps {
    fn click(&self, _node: &dyn Any) {}
    fn rect(&self, node: &dyn Any) -> ViewportRect {
        absolute_rect_of_node(node).unwrap_or(ViewportRect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 })
    }
}
pub(crate) static MACOS_PRESSABLE_OPS: MacosPressableOps = MacosPressableOps;

/// Build a [`ButtonHandle`] backed by the node so it can be an anchor target.
pub(crate) fn make_button_handle(node: &MacosNode) -> runtime_core::ButtonHandle {
    runtime_core::ButtonHandle::new(Rc::new(node.clone()) as Rc<dyn Any>, &MACOS_BUTTON_OPS)
}

/// Build a [`PressableHandle`] backed by the node so it can be an anchor target.
pub(crate) fn make_pressable_handle(node: &MacosNode) -> runtime_core::PressableHandle {
    runtime_core::PressableHandle::new(Rc::new(node.clone()) as Rc<dyn Any>, &MACOS_PRESSABLE_OPS)
}

// =========================================================================
// ScrollView ops
// =========================================================================

/// `ScrollViewOps` for macOS — programmatic scrolling (`scroll_to`) by moving
/// the `NSScrollView`'s clip view. Without it `ScrollViewHandle::scroll_to`
/// dispatches through the no-op default and silently does nothing (so e.g.
/// drag-to-edge autoscroll never moves). Interoperable with the web/iOS
/// `scroll_to` — same content-pixel coordinate space.
pub(crate) struct MacosScrollViewOps;

impl runtime_core::primitives::scroll_view::ScrollViewOps for MacosScrollViewOps {
    fn scroll_to(&self, node: &dyn Any, x: f32, y: f32) {
        let Some(macos_node) = node.downcast_ref::<MacosNode>() else {
            return;
        };
        let scroll = macos_node.as_view(); // the NSScrollView
        // Scroll the clip view (contentView) and reflect it so the scroller +
        // documentView placement update. The documentView is `isFlipped`
        // (top-left coords, matching Taffy/web), so (x, y) maps directly —
        // `x` for the horizontal scroller. `setFrame:` on the documentView
        // alone wouldn't move it; AppKit needs `reflectScrolledClipView:`.
        let clip: *mut objc2::runtime::AnyObject = unsafe { msg_send![scroll, contentView] };
        if clip.is_null() {
            return;
        }
        let point = objc2_foundation::CGPoint {
            x: x as f64,
            y: y as f64,
        };
        let _: () = unsafe { msg_send![clip, scrollToPoint: point] };
        let _: () = unsafe { msg_send![scroll, reflectScrolledClipView: clip] };
    }
}

pub(crate) static MACOS_SCROLL_OPS: MacosScrollViewOps = MacosScrollViewOps;

/// Build a [`ScrollViewHandle`] for `node` backed by [`MacosScrollViewOps`].
pub(crate) fn make_scroll_view_handle(
    node: &MacosNode,
) -> runtime_core::primitives::scroll_view::ScrollViewHandle {
    runtime_core::primitives::scroll_view::ScrollViewHandle::new(
        Rc::new(node.clone()) as Rc<dyn Any>,
        &MACOS_SCROLL_OPS,
    )
}

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

/// Window/viewport-relative rect for a node, or `None` when the view isn't
/// mounted in a window yet. Shared by `ViewOps::rect` (overlay anchoring) and
/// `ViewOps::absolute_frame` so both agree on coordinate space.
fn absolute_rect_of_node(node: &dyn Any) -> Option<ViewportRect> {
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
    // For a `text_area` the node is the NSScrollView wrapper — focus must land
    // on the inner NSTextView, not the scroll view. `editable_text_target` is
    // identity for a single-line NSTextField.
    let target = crate::imp::editable_text_target(n.as_view());
    let target: &NSView = &target;
    unsafe {
        let window: *mut objc2::runtime::AnyObject = msg_send![target, window];
        if !window.is_null() {
            let _: bool = msg_send![window, makeFirstResponder: target];
        }
    }
}

/// Resign first responder (clears the caret), via the window.
fn resign_first_responder(node: &dyn Any) {
    let Some(n) = node.downcast_ref::<MacosNode>() else { return };
    let target = crate::imp::editable_text_target(n.as_view());
    let target: &NSView = &target;
    unsafe {
        let window: *mut objc2::runtime::AnyObject = msg_send![target, window];
        if !window.is_null() {
            let nil: *mut objc2::runtime::AnyObject = std::ptr::null_mut();
            let _: bool = msg_send![window, makeFirstResponder: nil];
        }
    }
}

/// Select the field's whole contents (`-[NSText selectAll:]`).
fn select_all_text(node: &dyn Any) {
    let Some(n) = node.downcast_ref::<MacosNode>() else { return };
    let target = crate::imp::editable_text_target(n.as_view());
    let nil: *mut objc2::runtime::AnyObject = std::ptr::null_mut();
    unsafe {
        let _: () = msg_send![&*target, selectAll: nil];
    }
}

/// Insert `text` at the caret (best-effort; the controlling `Signal` stays the
/// source of truth via the widget's normal change notification).
fn insert_text_into(node: &dyn Any, text: &str) {
    let Some(n) = node.downcast_ref::<MacosNode>() else { return };
    let target = crate::imp::editable_text_target(n.as_view());
    let s = NSString::from_str(text);
    unsafe {
        let _: () = msg_send![&*target, insertText: &*s];
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

// Regression test for the tooltip/popover "anchors to window top-left" bug.
//
// `ViewOps::rect` is the anchor rect the portal placement math reads (see
// `runtime_core::primitives::portal`). Its contract is VIEWPORT/WINDOW-relative
// coordinates. macOS used to return the PARENT-relative `view.frame` here, so a
// trigger nested below the window root reported coordinates in its parent's
// space; placement treated those as window coordinates and pinned every
// overlay to (0, 0) — the window's top-left.
//
// This guards that `rect()` returns the accumulated window-relative origin
// (matching `absolute_frame`), NOT the single parent-relative `frame`.
//
// Why a live-AppKit test rather than pure geometry (cf. `private_layer_hittest`,
// which extracts pure math): the conversion here IS one AppKit call
// (`convertRect:toView:`) with no Rust-side decomposition to unit-test — the
// view-tree walk and per-level flip handling live inside AppKit. The closest
// reachable coverage is to build a real nested view hierarchy in a window and
// assert the two coordinate spaces diverge as the contract requires.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGSize, MainThreadMarker};

    use crate::imp::FlippedView;

    // All views are `FlippedView` (top-left origin, y-down) so window-relative
    // origins are a simple sum of parent-relative origins — matching the
    // `isFlipped` content view the host installs in production.
    fn flipped(mtm: MainThreadMarker, x: f64, y: f64, w: f64, h: f64) -> Retained<NSView> {
        let v = FlippedView::new(mtm);
        let frame = CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } };
        let _: () = unsafe { msg_send![&v, setFrame: frame] };
        // Expose as the super type (`NSView`) the ops downcast against.
        Retained::into_super(v)
    }

    #[test]
    fn rect_is_window_relative_not_parent_relative() {
        // AppKit raises an Objective-C exception (→ SIGABRT, "Rust cannot catch
        // foreign exceptions") if you create an `NSWindow` / drive view geometry
        // off the main thread without a running `NSApplication`. `cargo test`
        // runs every case on a spawned worker thread, so this assertion can only
        // execute when the binary is invoked such that tests land on the main
        // thread. Skip otherwise rather than abort the whole test binary — the
        // same hard boundary documented in `private_layer_hittest.rs` (which is
        // why THAT module tests pure geometry instead). When this does run on
        // the main thread it is a true fail-before/pass-after guard for the
        // tooltip/popover "anchored to window top-left" regression.
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        if unsafe { pthread_main_np() } == 0 {
            eprintln!(
                "skipping rect_is_window_relative_not_parent_relative: not on the \
                 main thread (AppKit geometry needs it; see comment)"
            );
            return;
        }

        // The offscreen geometry built here is self-contained (no run loop, no
        // other thread touches these views), so the unchecked marker is sound.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };

        // Window with a FLIPPED content view (matches the host chrome).
        let content_rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: 800.0, height: 600.0 },
        };
        let window: Retained<NSWindow> = unsafe {
            let alloc = mtm.alloc::<NSWindow>();
            NSWindow::initWithContentRect_styleMask_backing_defer(
                alloc,
                content_rect,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::NSBackingStoreBuffered,
                false,
            )
        };
        let content = flipped(mtm, 0.0, 0.0, 800.0, 600.0);
        window.setContentView(Some(&content));

        // child at (50, 60) in the content view; grandchild at (10, 20) within
        // the child. Grandchild's window origin is therefore (60, 80).
        let child = flipped(mtm, 50.0, 60.0, 200.0, 150.0);
        unsafe { content.addSubview(&child) };
        let grandchild = flipped(mtm, 10.0, 20.0, 80.0, 40.0);
        unsafe { child.addSubview(&grandchild) };

        let node = MacosNode::View(grandchild);
        let ops = MacosViewOps;

        // Parent-relative frame is unchanged: the grandchild's own (10, 20).
        let frame = ops.frame(&node).expect("frame in a mounted window");
        assert_eq!((frame.x, frame.y), (10.0, 20.0));

        // The fix: the anchor rect is WINDOW-relative — origins accumulate.
        let rect = ops.rect(&node);
        assert_eq!(
            (rect.x, rect.y),
            (60.0, 80.0),
            "rect() must be window-relative (50+10, 60+20); the bug returned \
             the parent-relative frame, pinning overlays to the window origin"
        );
        assert_eq!((rect.width, rect.height), (80.0, 40.0));

        // And it must agree with `absolute_frame`, the other window-space path
        // (drag-and-drop hit-testing) — they share one helper now.
        let abs = ops.absolute_frame(&node).expect("absolute_frame in a window");
        assert_eq!((rect.x, rect.y), (abs.x, abs.y));

        // Distinguishing assertion: pre-fix `rect() == frame()` (both parent-
        // relative). Post-fix they diverge for a nested trigger.
        assert_ne!((rect.x, rect.y), (frame.x, frame.y));
    }
}
