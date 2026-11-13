//! The placeholder ink on real AppKit
//! (`TextInputOps::update_text_input_placeholder_color`,
//! `update_text_area_placeholder_color`), read from a rendered bitmap.
//!
//! The input's ink rides an override of its cell's
//! `placeholderAttributedString` getter, so only a drawing shows whether
//! AppKit actually asks the getter: a property read would pass either way.
//!
//! `harness = false` for the same reason as `focus_attach`: AppKit views are
//! main-thread only, and libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test placeholder_color
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
    use objc2::{msg_send, msg_send_id, ClassType};
    use objc2_app_kit::{NSTextView, NSView};
    use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker, NSObject};
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{Color, Length, StyleRules, Tokenized};
    use runtime_vocabulary::caps::{StyleOps, TextInputOps};

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    fn set_frame(view: &NSView) {
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 300.0, height: 40.0 } };
        let _: () = unsafe { msg_send![view, setFrame: frame] };
    }

    /// Pixels of `view`, drawn now, that are clearly red: the test ink.
    fn red_pixels(view: &NSView) -> usize {
        let bounds: CGRect = unsafe { msg_send![view, bounds] };
        let rep: Retained<NSObject> =
            unsafe { msg_send_id![view, bitmapImageRepForCachingDisplayInRect: bounds] };
        let _: () = unsafe { msg_send![view, cacheDisplayInRect: bounds, toBitmapImageRep: &*rep] };
        let w: isize = unsafe { msg_send![&rep, pixelsWide] };
        let h: isize = unsafe { msg_send![&rep, pixelsHigh] };
        let mut n = 0;
        for x in 0..w {
            for y in 0..h {
                let c: Option<Retained<NSObject>> = unsafe { msg_send_id![&rep, colorAtX: x, y: y] };
                let Some(c) = c else { continue };
                let (r, g, b, a): (CGFloat, CGFloat, CGFloat, CGFloat) = unsafe {
                    (
                        msg_send![&c, redComponent],
                        msg_send![&c, greenComponent],
                        msg_send![&c, blueComponent],
                        msg_send![&c, alphaComponent],
                    )
                };
                if a > 0.5 && r > 0.6 && g < 0.3 && b < 0.3 {
                    n += 1;
                }
            }
        }
        n
    }

    fn find_text_view(view: &NSView) -> Option<Retained<NSView>> {
        let is_text_view: bool = unsafe { msg_send![view, isKindOfClass: NSTextView::class()] };
        if is_text_view {
            return unsafe { Retained::retain(view as *const NSView as *mut NSView) };
        }
        unsafe { view.subviews() }.iter().find_map(|sub| find_text_view(sub))
    }

    fn red() -> Color {
        Color("#ff0000".into())
    }

    /// Regression: an empty input's placeholder had no ink of its own, so
    /// AppKit drew it in `placeholderTextColor`, which follows the OS
    /// appearance rather than the app's theme. The ink draws, survives a
    /// secure swap (a cell swap), and `None` returns to AppKit's own.
    fn regression_text_input_placeholder_draws_in_the_ink(backend: &mut MacosBackend) {
        let node =
            backend.create_text_input("", Some("Placeholder"), Rc::new(|_| {}), None, None, false, &AccessibilityProps::default());
        let view = view_of(&node);
        set_frame(&view);
        assert_eq!(red_pixels(&view), 0, "AppKit's own placeholder ink is not red");

        backend.update_text_input_placeholder_color(&node, Some(&red()));
        assert!(red_pixels(&view) > 0, "the placeholder draws in the ink");

        // A restyle sets a new font, which an attributed placeholder does not
        // take from the cell: the ink must be rebuilt, not dropped.
        backend.apply_style(
            &node,
            &Rc::new(StyleRules { font_size: Some(Tokenized::Literal(Length::Px(20.0))), ..Default::default() }),
        );
        assert!(red_pixels(&view) > 0, "the ink survives a restyle's font");

        backend.update_text_input_secure(&node, true);
        assert!(red_pixels(&view) > 0, "the ink survives the secure cell swap");
        backend.update_text_input_secure(&node, false);
        assert!(red_pixels(&view) > 0, "and the swap back");

        backend.update_text_input_placeholder(&node, Some("Other"));
        assert!(red_pixels(&view) > 0, "a new placeholder text keeps the ink");

        backend.update_text_input_placeholder_color(&node, None);
        assert_eq!(red_pixels(&view), 0, "None returns to AppKit's own ink");
    }

    /// The text area draws its own placeholder; it takes the ink too.
    fn text_area_placeholder_draws_in_the_ink(backend: &mut MacosBackend) {
        let node = backend.create_text_area(
            "",
            Some("Placeholder"),
            true,
            None,
            None,
            Rc::new(|_| {}),
            None,
            &AccessibilityProps::default(),
        );
        let inner = find_text_view(&view_of(&node)).expect("a text area holds an NSTextView");
        set_frame(&inner);
        assert_eq!(red_pixels(&inner), 0, "AppKit's own placeholder ink is not red");

        backend.update_text_area_placeholder_color(&node, Some(&red()));
        assert!(red_pixels(&inner) > 0, "the placeholder draws in the ink");

        backend.update_text_area_placeholder_color(&node, None);
        assert_eq!(red_pixels(&inner), 0, "None returns to AppKit's own ink");
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(unsafe { pthread_main_np() } != 0, "placeholder_color must run on the main thread (harness = false)");
        // SAFETY: checked just above: this is the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        let _app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        let mut backend = MacosBackend::new(mtm);
        regression_text_input_placeholder_draws_in_the_ink(&mut backend);
        println!("test regression_text_input_placeholder_draws_in_the_ink ... ok");
        text_area_placeholder_draws_in_the_ink(&mut backend);
        println!("test text_area_placeholder_draws_in_the_ink ... ok");
        println!("placeholder_color: 2 passed");
    }
}
