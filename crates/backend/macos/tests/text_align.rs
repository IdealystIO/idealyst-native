//! `text_align` against real AppKit: where the glyphs actually land for
//! `text`, styled `text`, `text_input` and `text_area`, and that a style with no
//! alignment hands the control back to `NSTextAlignmentNatural`.
//!
//! Ink is read from a rendered bitmap, because the `alignment` property alone
//! can't catch a wrong raw value: it reads back whatever integer was written.
//!
//! `harness = false` for the same reason as `focus_attach`: AppKit views and
//! windows are main-thread only, and libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test text_align
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
    use objc2_app_kit::{
        NSBackingStoreType, NSTextAlignment, NSTextView, NSView, NSWindow, NSWindowStyleMask,
    };
    use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker, NSObject, NSRange, NSString};
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{Color, Length, StyleRules, TextAlign, TextRun, TextRunStyle, Tokenized};
    use runtime_vocabulary::caps::{StyleOps, TextInputOps, TextOps};

    const WIDTH: f64 = 400.0;
    const HEIGHT: f64 = 60.0;

    fn rect() -> CGRect {
        CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: WIDTH, height: HEIGHT } }
    }

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    /// Black text, so ink is told from the field's surface whatever its colour.
    fn aligned(align: Option<TextAlign>) -> Rc<StyleRules> {
        Rc::new(StyleRules {
            text_align: align,
            color: Some(Tokenized::Literal(Color("#000000".into()))),
            ..Default::default()
        })
    }

    /// Leftmost and rightmost dark column of `view` drawn at its current frame.
    fn ink(view: &NSView) -> (f64, f64) {
        let bounds: CGRect = unsafe { msg_send![view, bounds] };
        let rep: Retained<NSObject> =
            unsafe { msg_send_id![view, bitmapImageRepForCachingDisplayInRect: bounds] };
        let _: () = unsafe { msg_send![view, cacheDisplayInRect: bounds, toBitmapImageRep: &*rep] };
        let w: isize = unsafe { msg_send![&rep, pixelsWide] };
        let h: isize = unsafe { msg_send![&rep, pixelsHigh] };
        let (mut first, mut last) = (None, None);
        for x in 0..w {
            for y in 0..h {
                let c: Option<Retained<NSObject>> = unsafe { msg_send_id![&rep, colorAtX: x, y: y] };
                let Some(c) = c else { continue };
                let (r, a): (CGFloat, CGFloat) =
                    unsafe { (msg_send![&c, redComponent], msg_send![&c, alphaComponent]) };
                if a > 0.5 && r < 0.5 {
                    first.get_or_insert(x);
                    last = Some(x);
                    break;
                }
            }
        }
        let scale = w as f64 / bounds.size.width;
        (
            first.expect("the view drew no text") as f64 / scale,
            last.expect("the view drew no text") as f64 / scale,
        )
    }

    /// Asserts the ink sits where `align` puts it in a `WIDTH`-wide box. `None`
    /// and `Left` both start at the leading edge for this left-to-right text.
    fn assert_ink(what: &str, view: &NSView, align: Option<TextAlign>) {
        let (first, last) = ink(view);
        let middle = (first + last) / 2.0;
        match align {
            None | Some(TextAlign::Left) | Some(TextAlign::Justify) => {
                assert!(first < WIDTH * 0.2, "{what}: {align:?} ink starts at the left, got {first}..{last}")
            }
            Some(TextAlign::Center) => {
                assert!((middle - WIDTH / 2.0).abs() < WIDTH * 0.1, "{what}: {align:?} ink is centred, got {first}..{last}")
            }
            Some(TextAlign::Right) => {
                assert!(last > WIDTH * 0.8, "{what}: {align:?} ink ends at the right, got {first}..{last}")
            }
        }
    }

    fn alignment(view: &NSView) -> NSTextAlignment {
        let raw: isize = unsafe { msg_send![view, alignment] };
        NSTextAlignment(raw)
    }

    fn set_frame(view: &NSView) {
        let _: () = unsafe { msg_send![view, setFrame: rect()] };
    }

    /// The `NSTextView` inside a `text_area`'s chrome container.
    fn find_text_view(view: &NSView) -> Option<Retained<NSView>> {
        let is_text_view: bool = unsafe { msg_send![view, isKindOfClass: NSTextView::class()] };
        if is_text_view {
            return unsafe { Retained::retain(view as *const NSView as *mut NSView) };
        }
        unsafe { view.subviews() }.iter().find_map(|sub| find_text_view(sub))
    }

    fn inner_text_view(view: &NSView) -> Retained<NSView> {
        find_text_view(view).expect("no NSTextView in this view")
    }

    fn regression_text_align_removed_keeps_alignment_on_a_label(backend: &mut MacosBackend) {
        let node = backend.create_text("Twelve", &AccessibilityProps::default());
        let view = view_of(&node);
        set_frame(&view);
        assert_eq!(alignment(&view), NSTextAlignment::Natural, "a new label is natural");

        backend.apply_style(&node, &aligned(Some(TextAlign::Right)));
        assert_ink("label", &view, Some(TextAlign::Right));

        backend.apply_style(&node, &aligned(None));
        assert_eq!(alignment(&view), NSTextAlignment::Natural, "no alignment is natural, not Left");
        assert_ink("label", &view, None);
    }

    fn regression_text_align_removed_keeps_alignment_on_an_input(backend: &mut MacosBackend) {
        let node = backend.create_text_input("Twelve", None, Rc::new(|_| {}), None, None, false, &AccessibilityProps::default());
        let view = view_of(&node);
        set_frame(&view);
        assert_eq!(alignment(&view), NSTextAlignment::Natural, "a new input is natural");

        backend.apply_style(&node, &aligned(Some(TextAlign::Right)));
        assert_ink("input", &view, Some(TextAlign::Right));

        backend.apply_style(&node, &aligned(None));
        assert_eq!(alignment(&view), NSTextAlignment::Natural, "no alignment is natural, not Left");
        assert_ink("input", &view, None);
    }

    /// `NSTextAlignmentRight` and `Center` are 2 and 1 on arm64 and the other way
    /// round on x86_64 only, so a hand-copied pair of constants is wrong on one.
    fn regression_text_align_right_and_center_were_swapped_on_arm64(backend: &mut MacosBackend) {
        let node = backend.create_text("Twelve", &AccessibilityProps::default());
        let view = view_of(&node);
        set_frame(&view);
        for align in [TextAlign::Right, TextAlign::Center, TextAlign::Left] {
            backend.apply_style(&node, &aligned(Some(align)));
            assert_ink("label", &view, Some(align));
        }
        let input = backend.create_text_input("Twelve", None, Rc::new(|_| {}), None, None, false, &AccessibilityProps::default());
        let input_view = view_of(&input);
        set_frame(&input_view);
        for align in [TextAlign::Right, TextAlign::Center, TextAlign::Left] {
            backend.apply_style(&input, &aligned(Some(align)));
            assert_ink("input", &input_view, Some(align));
        }
    }

    /// Secure entry is a cell swap, and the alignment lives on the cell.
    fn regression_secure_toggle_drops_text_align(backend: &mut MacosBackend) {
        let node = backend.create_text_input("Twelve", None, Rc::new(|_| {}), None, None, false, &AccessibilityProps::default());
        let view = view_of(&node);
        backend.apply_style(&node, &aligned(Some(TextAlign::Right)));
        backend.update_text_input_secure(&node, true);
        assert_eq!(alignment(&view), NSTextAlignment::Right, "secure on keeps it");
        backend.update_text_input_secure(&node, false);
        assert_eq!(alignment(&view), NSTextAlignment::Right, "secure off keeps it");
    }

    fn text_area_aligns_its_lines_and_goes_back_to_natural(backend: &mut MacosBackend) {
        let node = backend.create_text_area("One\nTwo", None, true, None, None, Rc::new(|_| {}), None, &AccessibilityProps::default());
        let inner = inner_text_view(&view_of(&node));
        set_frame(&inner);

        for align in [TextAlign::Right, TextAlign::Center] {
            backend.apply_style(&node, &aligned(Some(align)));
            assert_ink("text area", &inner, Some(align));
        }

        // A value written while aligned follows it too, and so does a field that
        // starts empty, which holds its alignment only in the typing attributes.
        backend.apply_style(&node, &aligned(Some(TextAlign::Right)));
        backend.update_text_area_value(&node, "Three");
        assert_ink("text area after a value write", &inner, Some(TextAlign::Right));

        backend.apply_style(&node, &aligned(None));
        assert_eq!(alignment(&inner), NSTextAlignment::Natural, "no alignment is natural");
        assert_ink("text area", &inner, None);

        let empty = backend.create_text_area("", None, true, None, None, Rc::new(|_| {}), None, &AccessibilityProps::default());
        let inner = inner_text_view(&view_of(&empty));
        set_frame(&inner);
        backend.apply_style(&empty, &aligned(Some(TextAlign::Right)));
        let typed = NSString::from_str("Typed");
        let _: () = unsafe { msg_send![&inner, insertText: &*typed] };
        assert_ink("typed into an aligned empty text area", &inner, Some(TextAlign::Right));
    }

    /// The attributed string carries no paragraph style of its own, so the label's
    /// alignment has to reach it, and each rebuild of the string must keep it.
    fn regression_text_align_ignored_by_styled_text(backend: &mut MacosBackend) {
        let runs = [TextRun::plain("Twelve "), TextRun::plain("runs")];
        let node = backend.create_styled_text(&runs, &AccessibilityProps::default());
        let view = view_of(&node);
        set_frame(&view);

        backend.apply_style(&node, &aligned(Some(TextAlign::Right)));
        assert_ink("styled text", &view, Some(TextAlign::Right));

        backend.update_styled_text(&node, &[TextRun::plain("Thirteen "), TextRun::plain("runs")]);
        assert_ink("styled text after a run update", &view, Some(TextAlign::Right));

        backend.apply_style(&node, &aligned(None));
        assert_ink("styled text", &view, None);
    }

    /// Writing the alignment after the string goes on must not flatten its runs.
    fn text_align_keeps_a_styled_runs_font(backend: &mut MacosBackend) {
        let big = TextRunStyle {
            font_size: Some(Tokenized::Literal(Length::Px(40.0))),
            ..Default::default()
        };
        let runs = [TextRun::plain("small "), TextRun::styled("big", big)];
        let node = backend.create_styled_text(&runs, &AccessibilityProps::default());
        let view = view_of(&node);
        backend.apply_style(&node, &aligned(Some(TextAlign::Right)));
        let string: Retained<NSObject> = unsafe { msg_send_id![&view, attributedStringValue] };
        let size_at = |index: usize| -> CGFloat {
            let key = NSString::from_str("NSFont");
            let font: Retained<NSObject> =
                unsafe { msg_send_id![&string, attribute: &*key, atIndex: index, effectiveRange: std::ptr::null_mut::<NSRange>()] };
            unsafe { msg_send![&font, pointSize] }
        };
        assert_eq!(size_at(7), 40.0, "the big run stays big");
        assert!(size_at(0) < 40.0, "the plain run stays at the base size");
    }

    /// `NSControl.setAlignment:` aborts an edit in progress, and `apply_style` runs
    /// whenever the focus ring flips. Only a real change may write it.
    fn regression_text_align_restyle_aborted_a_focused_edit(mtm: MainThreadMarker, backend: &mut MacosBackend) {
        let window: Retained<NSWindow> = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc::<NSWindow>(),
                rect(),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::NSBackingStoreBuffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        let content: Retained<NSView> = unsafe { msg_send_id![mtm.alloc::<NSView>(), initWithFrame: rect()] };
        window.setContentView(Some(&content));

        let node = backend.create_text_input("Twelve", None, Rc::new(|_| {}), None, None, false, &AccessibilityProps::default());
        let view = view_of(&node);
        set_frame(&view);
        unsafe { content.addSubview(&view) };
        window.makeKeyAndOrderFront(None);
        assert!(window.makeFirstResponder(Some(&view)), "the field takes focus");

        let editor = || -> Option<Retained<AnyObject>> { unsafe { msg_send_id![&view, currentEditor] } };
        let first = editor().expect("the field is being edited");
        let typed = NSString::from_str("X");
        let _: () = unsafe { msg_send![&first, insertText: &*typed] };

        for align in [None, Some(TextAlign::Right), Some(TextAlign::Right), None, Some(TextAlign::Right)] {
            backend.apply_style(&node, &aligned(align));
            assert!(editor().is_some(), "styling {align:?} keeps the field in edit");
            let shown: isize = unsafe { msg_send![&first, alignment] };
            assert_eq!(
                NSTextAlignment(shown),
                if align.is_some() { NSTextAlignment::Right } else { NSTextAlignment::Natural },
                "the field editor shows {align:?} right away"
            );
        }

        assert!(window.makeFirstResponder(None), "the field gives up focus");
        assert_eq!(alignment(&view), NSTextAlignment::Right, "the field keeps it once the edit ends");
        assert_ink("input after the edit", &view, Some(TextAlign::Right));

        let value: Retained<NSString> = unsafe { msg_send_id![&view, stringValue] };
        assert!(value.to_string().contains('X'), "typed text survives, got {value}");
        window.orderOut(None);
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(unsafe { pthread_main_np() } != 0, "text_align must run on the main thread (harness = false)");
        // SAFETY: checked just above: this is the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        let _app = objc2_app_kit::NSApplication::sharedApplication(mtm);
        let mut backend = MacosBackend::new(mtm);

        let mut failed = 0;
        let mut check = |name: &str, case: &mut dyn FnMut()| {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(case)) {
                Ok(()) => println!("test {name} ... ok"),
                Err(_) => {
                    println!("test {name} ... FAILED");
                    failed += 1;
                }
            }
        };
        check("regression_text_align_right_and_center_were_swapped_on_arm64", &mut || regression_text_align_right_and_center_were_swapped_on_arm64(&mut backend));
        check("regression_text_align_removed_keeps_alignment_on_a_label", &mut || regression_text_align_removed_keeps_alignment_on_a_label(&mut backend));
        check("regression_text_align_removed_keeps_alignment_on_an_input", &mut || regression_text_align_removed_keeps_alignment_on_an_input(&mut backend));
        check("regression_text_align_ignored_by_styled_text", &mut || regression_text_align_ignored_by_styled_text(&mut backend));
        check("text_align_keeps_a_styled_runs_font", &mut || text_align_keeps_a_styled_runs_font(&mut backend));
        check("regression_secure_toggle_drops_text_align", &mut || regression_secure_toggle_drops_text_align(&mut backend));
        check("text_area_aligns_its_lines_and_goes_back_to_natural", &mut || text_area_aligns_its_lines_and_goes_back_to_natural(&mut backend));
        check("regression_text_align_restyle_aborted_a_focused_edit", &mut || regression_text_align_restyle_aborted_a_focused_edit(mtm, &mut backend));
        assert_eq!(failed, 0, "{failed} text_align case(s) failed");
    }
}
