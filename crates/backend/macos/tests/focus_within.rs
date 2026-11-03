//! `view` / `pressable` `on_focus` against real AppKit
//! (`InputOps::install_focus_handler` → `imp::view::report_focus_within`).
//!
//! `harness = false`: AppKit builds windows only on the main thread (see
//! `focus_attach.rs`). The reports are deferred one run-loop tick, so each
//! step pumps the main run loop before reading them.
//!
//! These read the RAW backend reports (no vocabulary dedup in between): a
//! focus move between two children of one wrapper is `false` then `true`.
//!
//! ```sh
//! cargo test -p backend-macos --test focus_within
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::RefCell;
    use std::rc::Rc;

    use backend_macos::{MacosBackend, MacosNode};
    use objc2::rc::Retained;
    use objc2::{msg_send, msg_send_id};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_vocabulary::caps::{InputOps, PressableOps, TextInputOps, ViewOps};

    #[allow(non_upper_case_globals)]
    extern "C" {
        static kCFRunLoopDefaultMode: *const std::ffi::c_void;
        fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, return_after: u8) -> i32;
    }

    fn pump() {
        for _ in 0..5 {
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 1) };
        }
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

    type Log = Rc<RefCell<Vec<String>>>;

    fn watch(backend: &mut MacosBackend, node: &MacosNode, name: &'static str, log: &Log) {
        let log = log.clone();
        backend.install_focus_handler(node, Rc::new(move |f: bool| log.borrow_mut().push(format!("{name} {f}"))));
    }

    fn take(log: &Log) -> Vec<String> {
        std::mem::take(&mut *log.borrow_mut())
    }

    /// Programmatic focus (no pointer event in flight = keyboard focus) on a
    /// pressable reaches its own handler and its wrapping view's; moving to a
    /// sibling reports the old one leaving and the new one arriving; moving
    /// out of the wrapper reports it leaving.
    fn pressable_focus_reaches_itself_and_its_ancestors(mtm: MainThreadMarker, backend: &mut MacosBackend) {
        let log: Log = Rc::default();
        let wrap = backend.create_view(&AccessibilityProps::default());
        let a = backend.create_pressable(Rc::new(|| {}), &AccessibilityProps::default());
        let b = backend.create_pressable(Rc::new(|| {}), &AccessibilityProps::default());
        watch(backend, &wrap, "wrap", &log);
        watch(backend, &a, "a", &log);
        let (window, content) = window(mtm);
        unsafe {
            view_of(&wrap).addSubview(&view_of(&a));
            view_of(&wrap).addSubview(&view_of(&b));
            content.addSubview(&view_of(&wrap));
        }

        let _: bool = unsafe { msg_send![&window, makeFirstResponder: &*view_of(&a)] };
        pump();
        assert_eq!(take(&log), ["a true", "wrap true"]);

        let _: bool = unsafe { msg_send![&window, makeFirstResponder: &*view_of(&b)] };
        pump();
        assert_eq!(take(&log), ["a false", "wrap false", "wrap true"], "a resigns, b becomes");

        let _: bool = unsafe { msg_send![&window, makeFirstResponder: &*content] };
        pump();
        assert_eq!(take(&log), ["wrap false"]);
        window.close();
    }

    /// A text field inside the wrapper reports through its cell's editing
    /// hooks — the field editor, not the field, is first responder.
    fn text_field_focus_reaches_the_wrapper(mtm: MainThreadMarker, backend: &mut MacosBackend) {
        let log: Log = Rc::default();
        let wrap = backend.create_view(&AccessibilityProps::default());
        let field = backend.create_text_input("", None, Rc::new(|_| {}), None, None, false, &AccessibilityProps::default());
        watch(backend, &wrap, "wrap", &log);
        let (window, content) = window(mtm);
        unsafe {
            view_of(&wrap).addSubview(&view_of(&field));
            content.addSubview(&view_of(&wrap));
        }
        let _: bool = unsafe { msg_send![&window, makeFirstResponder: &*view_of(&field)] };
        pump();
        assert!(take(&log).iter().any(|l| l == "wrap true"), "field focus reaches the wrapper");
        let _: bool = unsafe { msg_send![&window, makeFirstResponder: &*content] };
        pump();
        assert_eq!(take(&log).last().map(String::as_str), Some("wrap false"), "leaving the field leaves the wrapper");
        window.close();
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(unsafe { pthread_main_np() } != 0, "focus_within must run on the main thread (harness = false)");
        // SAFETY: checked just above — this is the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        let mut backend = MacosBackend::new(mtm);
        let tests: [(&str, fn(MainThreadMarker, &mut MacosBackend)); 2] = [
            ("pressable_focus_reaches_itself_and_its_ancestors", pressable_focus_reaches_itself_and_its_ancestors),
            ("text_field_focus_reaches_the_wrapper", text_field_focus_reaches_the_wrapper),
        ];
        for (name, t) in tests {
            t(mtm, &mut backend);
            println!("test {name} ... ok");
        }
        println!("test result: ok. {} passed", tests.len());
    }
}
