//! Attach-safe `focus()` against real AppKit (`src/imp/pending_focus.rs`).
//!
//! `harness = false`: AppKit builds windows only on the main thread, and
//! libtest runs every case on a worker thread. This binary's `main` IS the
//! main thread, so the windows, `viewDidMoveToWindow` and the main-queue
//! microtask that applies the pending focus are all real.
//!
//! ```sh
//! cargo test -p backend-macos --test focus_attach
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use std::rc::Rc;

    use backend_macos::{MacosBackend, MacosNode};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id, ClassType};
    use objc2_app_kit::{NSBackingStoreType, NSTextField, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_vocabulary::caps::TextInputOps;

    #[allow(non_upper_case_globals)]
    extern "C" {
        static kCFRunLoopDefaultMode: *const std::ffi::c_void;
        fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, return_after: u8) -> i32;
    }

    /// Run the main run loop until `done` or the turn budget is spent —
    /// enough turns for the main-queue microtask the pending focus rides.
    fn pump_until(mut done: impl FnMut() -> bool) -> bool {
        const TURNS: usize = 50;
        for _ in 0..TURNS {
            if done() {
                return true;
            }
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 1) };
        }
        done()
    }

    fn window(mtm: MainThreadMarker) -> (Retained<NSWindow>, Retained<NSView>) {
        let rect = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 400.0, height: 300.0 } };
        let window: Retained<NSWindow> = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc::<NSWindow>(),
                rect,
                NSWindowStyleMask::Titled,
                NSBackingStoreType::NSBackingStoreBuffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        let content: Retained<NSView> = unsafe { msg_send_id![mtm.alloc::<NSView>(), initWithFrame: rect] };
        window.setContentView(Some(&content));
        (window, content)
    }

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    fn has_window(view: &NSView) -> bool {
        let w: *mut AnyObject = unsafe { msg_send![view, window] };
        !w.is_null()
    }

    /// Whether keyboard focus is inside `view`: the window's first
    /// responder is `view` or a descendant (a text area's inner text view),
    /// or — for an `NSTextField` — its field editor, whose delegate is the
    /// field.
    fn focused_in(window: &NSWindow, view: &NSView) -> bool {
        let fr: *mut AnyObject = unsafe { msg_send![window, firstResponder] };
        if fr.is_null() {
            return false;
        }
        let fr = unsafe { &*fr };
        let is_view: bool = unsafe { msg_send![fr, isKindOfClass: NSView::class()] };
        if is_view {
            let inside: bool = unsafe { msg_send![fr, isDescendantOf: view] };
            if inside {
                return true;
            }
        }
        let is_field: bool = unsafe { msg_send![view, isKindOfClass: NSTextField::class()] };
        if is_field {
            let editor: *mut AnyObject = unsafe { msg_send![view, currentEditor] };
            return !editor.is_null() && std::ptr::eq(editor as *const AnyObject, fr as *const AnyObject);
        }
        false
    }

    fn text_input(backend: &mut MacosBackend, secure: bool) -> MacosNode {
        backend.create_text_input("", None, Rc::new(|_| {}), None, None, secure, &AccessibilityProps::default())
    }

    fn text_area(backend: &mut MacosBackend) -> MacosNode {
        backend.create_text_area("", None, true, None, None, Rc::new(|_| {}), None, &AccessibilityProps::default())
    }

    /// REGRESSION (CrewForge command palette): `focus()` on a field with no
    /// window yet was a silent no-op; it now makes the field first
    /// responder once the field is put in a window.
    fn focus_before_window_lands_once_attached(mtm: MainThreadMarker, backend: &mut MacosBackend) {
        for (what, node) in [
            ("text_input", text_input(backend, false)),
            ("secure text_input", text_input(backend, true)),
            ("text_area", text_area(backend)),
        ] {
            let view = view_of(&node);
            assert!(!has_window(&view));
            match what {
                "text_area" => backend.make_text_area_handle(&node).focus(),
                _ => backend.make_text_input_handle(&node).focus(),
            }
            let (window, content) = window(mtm);
            assert!(!focused_in(&window, &view), "{what}: nothing focused before attach");
            unsafe { content.addSubview(&view) };
            assert!(
                pump_until(|| focused_in(&window, &view)),
                "{what}: focus() before the window must make it first responder once attached"
            );
            unsafe { view.removeFromSuperview() };
            window.close();
        }
    }

    /// A `blur()` before attach cancels the pending focus.
    fn blur_before_window_cancels(mtm: MainThreadMarker, backend: &mut MacosBackend) {
        let node = text_input(backend, false);
        let view = view_of(&node);
        let handle = backend.make_text_input_handle(&node);
        handle.focus();
        handle.blur();
        let (window, content) = window(mtm);
        unsafe { content.addSubview(&view) };
        pump_until(|| false);
        assert!(!focused_in(&window, &view), "a blur before attach cancels the pending focus");
        window.close();
    }

    /// An attached field still focuses immediately (unchanged path).
    fn focus_when_attached_is_immediate(mtm: MainThreadMarker, backend: &mut MacosBackend) {
        let node = text_input(backend, false);
        let view = view_of(&node);
        let (window, content) = window(mtm);
        unsafe { content.addSubview(&view) };
        backend.make_text_input_handle(&node).focus();
        assert!(focused_in(&window, &view), "in a window: focused synchronously");
        window.close();
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(unsafe { pthread_main_np() } != 0, "focus_attach must run on the main thread (harness = false)");
        // SAFETY: checked just above — this is the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        // The main-queue microtask the pending focus is applied on.
        backend_macos::install_scheduler();
        let mut backend = MacosBackend::new(mtm);
        let tests: [(&str, fn(MainThreadMarker, &mut MacosBackend)); 3] = [
            ("regression_focus_before_window_lands_once_attached", focus_before_window_lands_once_attached),
            ("blur_before_window_cancels", blur_before_window_cancels),
            ("focus_when_attached_is_immediate", focus_when_attached_is_immediate),
        ];
        for (name, t) in tests {
            t(mtm, &mut backend);
            println!("test {name} ... ok");
        }
        println!("test result: ok. {} passed", tests.len());
    }
}
