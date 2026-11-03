//! A `text_input`'s focus notifier, and a hardware Escape in a focused
//! field closing the topmost overlay, on a real `UITextField`.
//!
//! REGRESSION (robot E2E `pressed_keys_edit_a_field_and_enter_submits_it`
//! on iOS): after typing "Hix" into a field, a window-level Backspace left
//! it "Hix". iOS implemented no `set_text_input_focus_handler`, so the
//! field's `on_focus` never ran and the robot saw no field focused — the
//! key had no field to edit. The field's become/resignFirstResponder now
//! fire the notifier — a resign only when the field held focus — and the
//! robot reads `focused` from `isFirstResponder`.
//!
//! REGRESSION (robot E2E `escape_closes_a_modal_while_its_field_has_focus`,
//! real-keyboard half): UIKit has no Escape-to-dismiss for the window-level
//! views a portal mounts as, so an iPad keyboard's Escape left the modal
//! open. A focused field now offers an Escape `UIKeyCommand` while a portal
//! with an `on_dismiss` is open, and its action closes the topmost one.
//! UIKit's own routing of the key press to that command needs a real key
//! event, which a test process can't make; the test checks the command the
//! field offers and runs its action.
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator
//! and run the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test text_input_focus --no-run
//! xcrun simctl spawn booted target/aarch64-apple-ios-sim/debug/deps/text_input_focus-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use backend_ios::{IosBackend, IosNode};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSString};
    use objc2_ui_kit::{UIView, UIWindow};
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_vocabulary::caps::{IntrospectionOps, TextInputOps};
    use runtime_vocabulary::escape_dismiss;

    #[link(name = "UIKit", kind = "framework")]
    extern "C" {
        static UIKeyInputEscape: &'static NSString;
    }

    fn field_of(node: &IosNode) -> &AnyObject {
        match node {
            IosNode::TextField(f) => f,
            _ => panic!("a text input mounts a UITextField"),
        }
    }

    /// A key window holding `field`, so it can become first responder.
    fn window_with(mtm: MainThreadMarker, field: &AnyObject) -> Retained<UIWindow> {
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 400.0, height: 800.0 } };
        let window: Retained<UIWindow> = unsafe { msg_send_id![mtm.alloc::<UIWindow>(), initWithFrame: frame] };
        let _: () = unsafe { msg_send![&window, makeKeyAndVisible] };
        let field_frame = CGRect { origin: CGPoint { x: 10.0, y: 10.0 }, size: CGSize { width: 200.0, height: 40.0 } };
        let _: () = unsafe { msg_send![field, setFrame: field_frame] };
        let _: () = unsafe { msg_send![&window, addSubview: field as *const AnyObject as *const UIView] };
        window
    }

    fn regression_a_focused_text_input_reports_focus(mtm: MainThreadMarker) {
        let mut backend = IosBackend::new(mtm);
        let node = backend.create_text_input("", None, Rc::new(|_| {}), None, None, false, &AccessibilityProps::default());
        let seen: Rc<RefCell<Vec<bool>>> = Rc::default();
        {
            let seen = seen.clone();
            backend.set_text_input_focus_handler(&node, Rc::new(move |f| seen.borrow_mut().push(f)));
        }
        let field = field_of(&node);
        let _window = window_with(mtm, field);
        let became: bool = unsafe { msg_send![field, becomeFirstResponder] };
        assert!(became, "the field takes first responder");
        let _: bool = unsafe { msg_send![field, resignFirstResponder] };
        assert_eq!(seen.borrow().as_slice(), [true, false], "focus gained, then lost");
    }

    /// REGRESSION: the robot's `focused` came only from the focus
    /// notifier (never installed on iOS → always false); it is now the
    /// responder's own answer (`IntrospectionOps::is_focused`), for a
    /// `text_area` too. And `blur()` resigns unconditionally, so an
    /// unfocused field reported a blur that never happened.
    fn regression_focus_is_read_from_the_responder_and_only_real_losses_report(mtm: MainThreadMarker) {
        let mut backend = IosBackend::new(mtm);
        let a11y = AccessibilityProps::default();
        let input = backend.create_text_input("", None, Rc::new(|_| {}), None, None, false, &a11y);
        let area = backend.create_text_area("", None, true, None, None, Rc::new(|_| {}), None, &a11y);
        let seen: Rc<RefCell<Vec<bool>>> = Rc::default();
        {
            let seen = seen.clone();
            backend.set_text_input_focus_handler(&input, Rc::new(move |f| seen.borrow_mut().push(f)));
        }
        let field = field_of(&input);
        let window = window_with(mtm, field);
        let area_view: &AnyObject = area.as_view();
        let area_frame = CGRect { origin: CGPoint { x: 10.0, y: 60.0 }, size: CGSize { width: 200.0, height: 80.0 } };
        let _: () = unsafe { msg_send![area_view, setFrame: area_frame] };
        let _: () = unsafe { msg_send![&window, addSubview: area_view as *const AnyObject as *const UIView] };

        assert_eq!(backend.is_focused(&input), Some(false));
        let _: bool = unsafe { msg_send![field, resignFirstResponder] };
        assert!(seen.borrow().is_empty(), "an unfocused field reports no blur: {:?}", seen.borrow());

        let became: bool = unsafe { msg_send![field, becomeFirstResponder] };
        assert!(became);
        assert_eq!(backend.is_focused(&input), Some(true));
        assert_eq!(backend.is_focused(&area), Some(false));

        let became: bool = unsafe { msg_send![area_view, becomeFirstResponder] };
        assert!(became, "the text area takes first responder");
        assert_eq!(backend.is_focused(&area), Some(true));
        assert_eq!(backend.is_focused(&input), Some(false));
        assert_eq!(seen.borrow().as_slice(), [true, false], "focus moved to the area: one real loss");
        let _: bool = unsafe { msg_send![area_view, resignFirstResponder] };
    }

    /// The `keyCommands` a responder offers, as `(input, action)`.
    fn commands(responder: &AnyObject) -> Vec<(String, String)> {
        unsafe {
            let list: *mut AnyObject = msg_send![responder, keyCommands];
            if list.is_null() {
                return Vec::new();
            }
            let count: usize = msg_send![list, count];
            (0..count)
                .map(|i| {
                    let command: *mut AnyObject = msg_send![list, objectAtIndex: i];
                    let input: *mut NSString = msg_send![command, input];
                    let action: Option<objc2::runtime::Sel> = msg_send![command, action];
                    ((*input).to_string(), action.map(|a| a.name().to_string()).unwrap_or_default())
                })
                .collect()
        }
    }

    fn regression_escape_in_a_focused_field_closes_the_topmost_overlay(mtm: MainThreadMarker) {
        let mut backend = IosBackend::new(mtm);
        let a11y = AccessibilityProps::default();
        let input = backend.create_text_input("", None, Rc::new(|_| {}), None, None, false, &a11y);
        let area = backend.create_text_area("", None, true, None, None, Rc::new(|_| {}), None, &a11y);
        let escape = unsafe { UIKeyInputEscape.to_string() };
        for node in [&input, &area] {
            let responder: &AnyObject = node.as_view();
            assert!(
                !commands(responder).iter().any(|(i, _)| *i == escape),
                "no overlay open: Escape is left alone"
            );
        }

        let (lower, upper) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let hidden = Rc::new(Cell::new(false));
        let l = lower.clone();
        let _lower = escape_dismiss::register(Rc::new(move || l.set(l.get() + 1)), hidden.clone());
        let u = upper.clone();
        let top = escape_dismiss::register(Rc::new(move || u.set(u.get() + 1)), hidden);
        for node in [&input, &area] {
            let responder: &AnyObject = node.as_view();
            let offered = commands(responder);
            assert!(
                offered.contains(&(escape.clone(), "idealystEscapeDismiss:".to_string())),
                "an overlay open: the field offers Escape ({offered:?})"
            );
            let nil: *mut AnyObject = std::ptr::null_mut();
            let _: () = unsafe { msg_send![responder, idealystEscapeDismiss: nil] };
        }
        assert_eq!((lower.get(), upper.get()), (0, 2), "Escape closes only the topmost overlay");
        drop(top);
        let _: () = unsafe { msg_send![input.as_view(), idealystEscapeDismiss: std::ptr::null_mut::<AnyObject>()] };
        assert_eq!((lower.get(), upper.get()), (1, 2), "then the one beneath");
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        regression_a_focused_text_input_reports_focus(mtm);
        regression_escape_in_a_focused_field_closes_the_topmost_overlay(mtm);
        regression_focus_is_read_from_the_responder_and_only_real_losses_report(mtm);
        println!("text_input_focus: 3 passed");
    }
}
