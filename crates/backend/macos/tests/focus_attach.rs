//! Attach-safe `focus()` against real AppKit (`src/imp/pending_focus.rs`),
//! and `set_disabled` taking controls out of keyboard focus.
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
    use runtime_vocabulary::caps::{PressableOps, SliderOps, StyleOps, TextInputOps, ToggleOps};

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

    fn accepts_first_responder(view: &NSView) -> bool {
        unsafe { msg_send![view, acceptsFirstResponder] }
    }

    /// `set_disabled` takes a text control out of keyboard focus: a field
    /// focused when it goes inert loses first responder, a disabled field
    /// refuses `focus()`, and re-enabling makes it focusable again. Before
    /// macOS implemented `set_disabled` (the trait default was a no-op) a
    /// disabled `Field` / `Textarea` stayed editable and focused.
    fn regression_disabled_text_controls_refuse_focus(mtm: MainThreadMarker, backend: &mut MacosBackend) {
        for (what, node) in [
            ("text_input", text_input(backend, false)),
            ("secure text_input", text_input(backend, true)),
            ("text_area", text_area(backend)),
        ] {
            let view = view_of(&node);
            let (window, content) = window(mtm);
            unsafe { content.addSubview(&view) };
            let focus = |backend: &mut MacosBackend| match what {
                "text_area" => backend.make_text_area_handle(&node).focus(),
                _ => backend.make_text_input_handle(&node).focus(),
            };
            focus(backend);
            assert!(focused_in(&window, &view), "{what}: precondition — an enabled field focuses");

            backend.set_disabled(&node, true);
            assert!(!focused_in(&window, &view), "{what}: going disabled drops focus");
            focus(backend);
            pump_until(|| false);
            assert!(!focused_in(&window, &view), "{what}: a disabled field cannot be focused");

            backend.set_disabled(&node, false);
            focus(backend);
            assert!(
                pump_until(|| focused_in(&window, &view)),
                "{what}: re-enabled, the field is focusable again"
            );
            unsafe { view.removeFromSuperview() };
            window.close();
        }
    }

    /// A disabled pressable leaves the key-view loop like a disabled
    /// `NSButton`: it stops accepting first responder, and if it held focus
    /// it gives it back to the window. Before, `set_disabled` was a no-op on
    /// macOS and a disabled Button / Switch / Select trigger stayed a Tab
    /// stop whose Space/Return did nothing.
    fn regression_disabled_pressable_leaves_key_view_loop(mtm: MainThreadMarker, backend: &mut MacosBackend) {
        let node = backend.create_pressable(Rc::new(|| {}), &AccessibilityProps::default());
        let view = view_of(&node);
        let (window, content) = window(mtm);
        unsafe { content.addSubview(&view) };
        assert!(accepts_first_responder(&view), "precondition: an enabled pressable is focusable");
        let took: bool = unsafe { msg_send![&window, makeFirstResponder: &*view] };
        assert!(took && focused_in(&window, &view), "precondition: it takes focus");

        backend.set_disabled(&node, true);
        assert!(!accepts_first_responder(&view), "a disabled pressable refuses first responder");
        assert!(!focused_in(&window, &view), "going disabled drops focus");
        // (`makeFirstResponder:`'s BOOL is not the signal: AppKit reports
        // YES once it falls back to making the window first responder. Where
        // focus actually landed is.)
        let _: bool = unsafe { msg_send![&window, makeFirstResponder: &*view] };
        assert!(!focused_in(&window, &view), "a disabled pressable cannot be focused");

        backend.set_disabled(&node, false);
        assert!(accepts_first_responder(&view), "re-enabled, it is focusable again");
        window.close();
    }

    /// `toggle` / `slider` go inert through `set_disabled` like every other
    /// `NSControl`: `setEnabled:NO` (the switch / knob can't be moved), no
    /// first responder, and focus dropped if held. Before the vocabulary
    /// bound `disabled` on these primitives this branch was unreachable;
    /// this pins that the AppKit half really makes `NSSwitch` / `NSSlider`
    /// inert.
    fn disabled_toggle_and_slider_are_not_enabled(mtm: MainThreadMarker, backend: &mut MacosBackend) {
        let toggle = backend.create_toggle(false, Rc::new(|_| {}), &AccessibilityProps::default());
        let slider = backend.create_slider(0.5, 0.0, 1.0, None, Rc::new(|_| {}), &AccessibilityProps::default());
        for (what, node) in [("toggle", toggle), ("slider", slider)] {
            let view = view_of(&node);
            let (window, content) = window(mtm);
            unsafe { content.addSubview(&view) };
            let enabled = |v: &NSView| -> bool { unsafe { msg_send![v, isEnabled] } };
            assert!(enabled(&view), "{what}: precondition — enabled");
            // Whether an enabled NSSwitch / NSSlider accepts first responder
            // follows the system "keyboard navigation" setting; when it does
            // take focus here, going disabled must drop it.
            let _: bool = unsafe { msg_send![&window, makeFirstResponder: &*view] };
            let held = focused_in(&window, &view);

            backend.set_disabled(&node, true);
            assert!(!enabled(&view), "{what}: set_disabled(true) disables the NSControl");
            assert!(!accepts_first_responder(&view), "{what}: a disabled control refuses first responder");
            if held {
                assert!(!focused_in(&window, &view), "{what}: going disabled drops focus");
            }

            backend.set_disabled(&node, false);
            assert!(enabled(&view), "{what}: re-enabled");
            unsafe { view.removeFromSuperview() };
            window.close();
        }
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
        let tests: [(&str, fn(MainThreadMarker, &mut MacosBackend)); 6] = [
            ("regression_focus_before_window_lands_once_attached", focus_before_window_lands_once_attached),
            ("blur_before_window_cancels", blur_before_window_cancels),
            ("focus_when_attached_is_immediate", focus_when_attached_is_immediate),
            ("regression_disabled_text_controls_refuse_focus", regression_disabled_text_controls_refuse_focus),
            ("regression_disabled_pressable_leaves_key_view_loop", regression_disabled_pressable_leaves_key_view_loop),
            ("disabled_toggle_and_slider_are_not_enabled", disabled_toggle_and_slider_are_not_enabled),
        ];
        for (name, t) in tests {
            t(mtm, &mut backend);
            println!("test {name} ... ok");
        }
        println!("test result: ok. {} passed", tests.len());
    }
}
