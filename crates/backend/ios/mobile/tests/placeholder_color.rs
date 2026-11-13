//! The placeholder ink on real UIKit (`TextInputOps::update_text_input_placeholder_color`,
//! `update_text_area_placeholder_color`), and the text area's drawn
//! placeholder (UITextView has none of its own).
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator
//! and run the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test placeholder_color --no-run
//! xcrun simctl spawn booted target/aarch64-apple-ios-sim/debug/deps/placeholder_color-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use std::rc::Rc;

    use backend_ios::{IosBackend, IosNode};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id};
    use objc2_foundation::{CGFloat, MainThreadMarker, NSString};
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::Color;
    use runtime_vocabulary::caps::TextInputOps;

    fn rgba(color: &AnyObject) -> (f64, f64, f64, f64) {
        let (mut r, mut g, mut b, mut a): (CGFloat, CGFloat, CGFloat, CGFloat) = (0.0, 0.0, 0.0, 0.0);
        let ok: bool = unsafe { msg_send![color, getRed: &mut r, green: &mut g, blue: &mut b, alpha: &mut a] };
        assert!(ok, "color converts to RGB");
        (r, g, b, a)
    }

    fn is_red(color: &AnyObject) -> bool {
        let (r, g, b, a) = rgba(color);
        (r - 1.0).abs() < 0.01 && g < 0.01 && b < 0.01 && (a - 1.0).abs() < 0.01
    }

    /// The field's attributed placeholder: its string and its color at 0.
    fn placeholder_of(field: &AnyObject) -> Option<(String, Option<Retained<AnyObject>>)> {
        unsafe {
            let attributed: Option<Retained<AnyObject>> = msg_send_id![field, attributedPlaceholder];
            let attributed = attributed?;
            let s: Retained<NSString> = msg_send_id![&attributed, string];
            let key = NSString::from_str("NSColor");
            let color: Option<Retained<AnyObject>> =
                msg_send_id![&attributed, attribute: &*key, atIndex: 0usize, effectiveRange: std::ptr::null_mut::<objc2_foundation::NSRange>()];
            Some((s.to_string(), color))
        }
    }

    /// Regression: a placeholder had no ink of its own, so UIKit drew it in
    /// the OS appearance's gray — near-invisible in a dark-themed app on a
    /// light-mode phone. The ink sets, survives a text change (UIKit's plain
    /// `placeholder` setter drops it), and clears back to UIKit's own.
    fn regression_text_field_placeholder_takes_the_ink(mtm: MainThreadMarker) {
        let mut backend = IosBackend::new(mtm);
        let a11y = AccessibilityProps::default();
        let node = backend.create_text_input("", Some("Name"), Rc::new(|_| {}), None, None, false, &a11y);
        let IosNode::TextField(field) = &node else { panic!("a text input mounts a UITextField") };
        backend.update_text_input_placeholder_color(&node, Some(&Color("#ff0000".into())));
        let (text, color) = placeholder_of(field).expect("a placeholder");
        assert_eq!(text, "Name");
        assert!(is_red(&color.expect("an ink")), "the placeholder is drawn in the ink");

        backend.update_text_input_placeholder(&node, Some("Other"));
        let (text, color) = placeholder_of(field).expect("a placeholder");
        assert_eq!(text, "Other", "a live placeholder text updates on iOS");
        assert!(is_red(&color.expect("an ink")), "the ink survives a text change");

        backend.update_text_input_placeholder_color(&node, None);
        let (text, color) = placeholder_of(field).expect("a placeholder");
        assert_eq!(text, "Other");
        assert!(!color.map(|c| is_red(&c)).unwrap_or(false), "None returns to UIKit's own ink");
    }

    /// The placeholder label a text area draws, if any.
    fn placeholder_label(view: &AnyObject) -> Option<Retained<AnyObject>> {
        unsafe {
            let _: () = msg_send![view, layoutIfNeeded];
            let subviews: Retained<AnyObject> = msg_send_id![view, subviews];
            let count: usize = msg_send![&subviews, count];
            (0..count).find_map(|i| {
                let v: Retained<AnyObject> = msg_send_id![&subviews, objectAtIndex: i];
                let is_label: bool = msg_send![&v, isKindOfClass: objc2::class!(UILabel)];
                is_label.then_some(v)
            })
        }
    }

    /// Regression: the text area ignored its placeholder on iOS (UITextView
    /// has none). It draws one, hides it while there is text, and takes the
    /// ink.
    fn regression_text_area_draws_its_placeholder(mtm: MainThreadMarker) {
        let mut backend = IosBackend::new(mtm);
        let a11y = AccessibilityProps::default();
        let node = backend.create_text_area("", Some("Notes"), true, None, None, Rc::new(|_| {}), None, &a11y);
        let IosNode::TextView(view) = &node else { panic!("a text area mounts a UITextView") };
        let _: () = unsafe { msg_send![&**view, setFrame: objc2_foundation::CGRect::new(objc2_foundation::CGPoint::new(0.0, 0.0), objc2_foundation::CGSize::new(200.0, 80.0))] };
        let label = placeholder_label(view).expect("the text area draws a placeholder");
        let text: Retained<NSString> = unsafe { msg_send_id![&label, text] };
        assert_eq!(text.to_string(), "Notes");
        assert!(!unsafe { msg_send![&label, isHidden] }, "shown while empty");

        backend.update_text_area_placeholder_color(&node, Some(&Color("#ff0000".into())));
        let ink: Retained<AnyObject> = unsafe { msg_send_id![&label, textColor] };
        assert!(is_red(&ink), "the placeholder takes the ink");

        backend.update_text_area_value(&node, "typed");
        let label = placeholder_label(view).expect("still there");
        assert!(unsafe { msg_send![&label, isHidden] }, "hidden once there is text");
        backend.update_text_area_value(&node, "");
        let label = placeholder_label(view).expect("still there");
        assert!(!unsafe { msg_send![&label, isHidden] }, "back when the text is cleared");
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_ios::install_scheduler();
        regression_text_field_placeholder_takes_the_ink(mtm);
        println!("test regression_text_field_placeholder_takes_the_ink ... ok");
        regression_text_area_draws_its_placeholder(mtm);
        println!("test regression_text_area_draws_its_placeholder ... ok");
        println!("placeholder_color: 2 passed");
    }
}
