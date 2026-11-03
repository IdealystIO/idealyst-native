//! `view` `on_focus` on a real UIKit window: a text input inside the view
//! becoming / resigning first responder reaches the view's handler, and one
//! outside it does not (`imp/focus_within.rs`). Reads the RAW backend
//! reports; the vocabulary dedups them to transitions.
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator
//! and run the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test focus_within --no-run
//! xcrun simctl spawn booted target/aarch64-apple-ios-sim/debug/deps/focus_within-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use std::cell::RefCell;
    use std::rc::Rc;

    use backend_ios::IosBackend;
    use objc2::rc::Retained;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use objc2_ui_kit::{UIView, UIWindow};
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_vocabulary::caps::{InputOps, TextInputOps, ViewOps};

    #[allow(non_upper_case_globals)]
    extern "C" {
        static kCFRunLoopDefaultMode: *const std::ffi::c_void;
        fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, return_after: u8) -> i32;
    }

    fn pump() {
        for _ in 0..10 {
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 1) };
        }
    }

    fn frame(x: f64, y: f64, w: f64, h: f64) -> CGRect {
        CGRect { origin: CGPoint { x, y }, size: CGSize { width: w, height: h } }
    }

    fn view_on_focus_hears_its_text_inputs(mtm: MainThreadMarker) {
        let mut backend = IosBackend::new(mtm);
        let a11y = AccessibilityProps::default();
        let wrap = backend.create_view(&a11y);
        let a = backend.create_text_input("", None, Rc::new(|_| {}), None, None, false, &a11y);
        let b = backend.create_text_area("", None, true, None, None, Rc::new(|_| {}), None, &a11y);
        let outside = backend.create_text_input("", None, Rc::new(|_| {}), None, None, false, &a11y);
        let log: Rc<RefCell<Vec<bool>>> = Rc::default();
        {
            let log = log.clone();
            backend.install_focus_handler(&wrap, Rc::new(move |f| log.borrow_mut().push(f)));
        }
        let window: Retained<UIWindow> = unsafe { msg_send_id![mtm.alloc::<UIWindow>(), initWithFrame: frame(0.0, 0.0, 400.0, 800.0)] };
        let _: () = unsafe { msg_send![&window, makeKeyAndVisible] };
        unsafe {
            let w: &UIView = wrap.as_view();
            let _: () = msg_send![w, setFrame: frame(0.0, 0.0, 400.0, 200.0)];
            let _: () = msg_send![a.as_view(), setFrame: frame(10.0, 10.0, 200.0, 40.0)];
            let _: () = msg_send![b.as_view(), setFrame: frame(10.0, 60.0, 200.0, 80.0)];
            let _: () = msg_send![outside.as_view(), setFrame: frame(10.0, 300.0, 200.0, 40.0)];
            let _: () = msg_send![w, addSubview: a.as_view()];
            let _: () = msg_send![w, addSubview: b.as_view()];
            let _: () = msg_send![&window, addSubview: w];
            let _: () = msg_send![&window, addSubview: outside.as_view()];
        }
        let take = || std::mem::take(&mut *log.borrow_mut());

        let _: bool = unsafe { msg_send![a.as_view(), becomeFirstResponder] };
        pump();
        assert_eq!(take(), [true], "a field inside the view took focus");

        let _: bool = unsafe { msg_send![b.as_view(), becomeFirstResponder] };
        pump();
        assert_eq!(take(), [false, true], "field → text area, both inside (raw: leave, enter)");

        let _: bool = unsafe { msg_send![outside.as_view(), becomeFirstResponder] };
        pump();
        assert_eq!(take(), [false], "focus left the view; the outside field reports nothing to it");

        let _: bool = unsafe { msg_send![outside.as_view(), resignFirstResponder] };
        pump();
        assert_eq!(take(), Vec::<bool>::new());
        drop(window);
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_ios::install_scheduler();
        view_on_focus_hears_its_text_inputs(mtm);
        println!("focus_within: 1 passed");
    }
}
