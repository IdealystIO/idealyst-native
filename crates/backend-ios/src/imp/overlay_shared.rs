//! Helpers shared between the two overlay implementations
//! ([`overlay`](super::overlay) — viewport-anchored; and
//! [`anchored_overlay`](super::anchored_overlay) — element-anchored).

use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2_ui_kit::UIView;

/// Add a backend-owned container view to the host's window. Sized
/// via autoresizing mask so its bounds track the window's bounds —
/// Taffy uses this as the viewport-root frame for the overlay's
/// content tree.
pub(crate) fn mount_in_window(host_view: &UIView, container: &UIView) {
    let window: Option<Retained<UIView>> = unsafe { msg_send_id![host_view, window] };
    let Some(window) = window else {
        eprintln!("[ios-overlay] host view has no window — cannot mount");
        return;
    };
    let window_bounds: objc2_foundation::CGRect = unsafe { msg_send![&window, bounds] };
    let _: () = unsafe { msg_send![container, setFrame: window_bounds] };
    // flexibleWidth (2) | flexibleHeight (16).
    let _: () = unsafe { msg_send![container, setAutoresizingMask: 0x12u64] };
    unsafe { window.addSubview(container) };
}

/// Dispatch a Rust closure on the main queue's next runloop turn.
/// Used to defer container mounting / unmounting outside of the
/// framework's current `backend.borrow_mut()` window.
pub(crate) fn schedule_main<F: FnOnce() + 'static>(f: F) {
    extern "C" {
        static _dispatch_main_q: std::ffi::c_void;
        fn dispatch_async_f(
            queue: *const std::ffi::c_void,
            context: *mut std::ffi::c_void,
            work: extern "C" fn(*mut std::ffi::c_void),
        );
    }

    let boxed: Box<Box<dyn FnOnce()>> = Box::new(Box::new(f));
    let ctx = Box::into_raw(boxed) as *mut std::ffi::c_void;

    extern "C" fn trampoline(ctx: *mut std::ffi::c_void) {
        let boxed: Box<Box<dyn FnOnce()>> = unsafe { Box::from_raw(ctx as *mut _) };
        boxed();
    }

    unsafe {
        dispatch_async_f(
            &_dispatch_main_q as *const _ as *const std::ffi::c_void,
            ctx,
            trampoline,
        );
    }
}
