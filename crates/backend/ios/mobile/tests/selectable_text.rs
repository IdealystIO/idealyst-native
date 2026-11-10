//! `text(selectable = true)` on real UIKit objects.
//!
//! REGRESSION: no text could be selected on iOS — every `text` is a
//! `UILabel`, which has no selection. A selectable text is now a read-only
//! `UITextView` (`IdealystSelectableText`), and it has to render as the
//! label every other text is.
//!
//! Each case mounts the same style twice, as a plain text and as a
//! selectable one, through the backend's create / style / layout path, each
//! in its own content-sized cell (so the width rule is tested, not only the
//! height), and checks:
//!
//! - the laid-out frames match exactly;
//! - the two views draw their glyphs in the same place: the inked box of
//!   each (`renderInContext:` into a bitmap) agrees to within a pixel on
//!   every edge. Not pixel-for-pixel: a label draws through NSStringDrawing
//!   and a text view through TextKit, whose line heights differ by a
//!   fraction of a point, so a later line can sit a pixel lower (single
//!   lines and most styles come out identical; the count is printed);
//! - the selectable one is selectable and not editable, and once a range
//!   is selected UIKit offers Copy; the label offers nothing.
//!
//! A real long-press can't be synthesized from a test binary (no XCUITest
//! here), so the selection is set programmatically: the gesture itself is
//! UIKit's, on a view that is selectable.
//!
//! `harness = false`: UIKit is main-thread only. Build for the simulator and
//! run the binary there:
//!
//! ```sh
//! cargo test -p backend-ios-mobile --target aarch64-apple-ios-sim \
//!     --test selectable_text --no-run
//! xcrun simctl spawn <udid> target/aarch64-apple-ios-sim/debug/deps/selectable_text-<hash>
//! ```

#[cfg(not(target_os = "ios"))]
fn main() {}

#[cfg(target_os = "ios")]
fn main() {
    imp::run();
}

#[cfg(target_os = "ios")]
mod imp {
    use std::ffi::c_void;
    use std::rc::Rc;

    use backend_ios::{IosBackend, IosNode};
    use objc2::encode::{Encode, Encoding};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id, sel};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSRange};
    use objc2_ui_kit::{UIView, UIWindow};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{
        AlignItems, Color, FontStyle, FontWeight, Length, StyleRules, TextAlign, TextRun, TextRunStyle,
    };
    use runtime_vocabulary::caps::{IntrospectionOps, LifecycleOps, StyleOps, TextOps, ViewOps};

    #[repr(transparent)]
    struct CgContext(*mut c_void);
    unsafe impl Encode for CgContext {
        const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("CGContext", &[]));
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
        fn CGColorSpaceRelease(space: *mut c_void);
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: *mut c_void,
            bitmap_info: u32,
        ) -> *mut c_void;
        fn CGContextRelease(ctx: *mut c_void);
        fn CGContextTranslateCTM(ctx: *mut c_void, tx: f64, ty: f64);
        fn CGContextScaleCTM(ctx: *mut c_void, sx: f64, sy: f64);
    }

    /// `view` drawn at its own size into an RGBA buffer (1 px per point).
    fn render(view: &UIView) -> (usize, usize, Vec<u8>) {
        let bounds: CGRect = unsafe { msg_send![view, bounds] };
        let (w, h) = (bounds.size.width.ceil() as usize, bounds.size.height.ceil() as usize);
        let mut pixels = vec![0u8; w.max(1) * h.max(1) * 4];
        unsafe {
            let space = CGColorSpaceCreateDeviceRGB();
            // kCGImageAlphaPremultipliedLast
            let ctx = CGBitmapContextCreate(pixels.as_mut_ptr().cast(), w.max(1), h.max(1), 8, w.max(1) * 4, space, 1);
            // UIKit draws top-down: flip the bitmap context.
            CGContextTranslateCTM(ctx, 0.0, h as f64);
            CGContextScaleCTM(ctx, 1.0, -1.0);
            let layer: *mut AnyObject = msg_send![view, layer];
            let _: () = msg_send![layer, renderInContext: CgContext(ctx)];
            CGContextRelease(ctx);
            CGColorSpaceRelease(space);
        }
        (w, h, pixels)
    }

    fn rules(f: impl FnOnce(&mut StyleRules)) -> Rc<StyleRules> {
        let mut r = StyleRules::default();
        f(&mut r);
        Rc::new(r)
    }

    const LONG: &str = "A longer paragraph of text that has to wrap onto several lines inside the cell, gjpqy.";

    struct Case {
        name: &'static str,
        content: &'static str,
        runs: Option<Vec<TextRun>>,
        style: Rc<StyleRules>,
    }

    fn cases() -> Vec<Case> {
        let bold_run = {
            let mut s = TextRunStyle::default();
            s.font_weight = Some(FontWeight::Bold);
            s
        };
        vec![
            Case { name: "defaults", content: "Person 0-0 Lastname", runs: None, style: rules(|_| {}) },
            Case {
                name: "20pt bold",
                content: "Night shift",
                runs: None,
                style: rules(|r| {
                    r.font_size = Some(Length::Px(20.0).into());
                    r.font_weight = Some(FontWeight::Bold);
                }),
            },
            Case {
                name: "italic, colour",
                content: "Expected at 18:00",
                runs: None,
                style: rules(|r| {
                    r.font_style = Some(FontStyle::Italic);
                    r.color = Some(Color("#2255cc".into()).into());
                }),
            },
            Case { name: "wrap", content: LONG, runs: None, style: rules(|_| {}) },
            Case { name: "2 lines", content: LONG, runs: None, style: rules(|r| r.max_lines = Some(2)) },
            Case {
                name: "padding 8",
                content: "Padded label",
                runs: None,
                style: rules(|r| {
                    for side in [&mut r.padding_left, &mut r.padding_right, &mut r.padding_top, &mut r.padding_bottom] {
                        *side = Some(Length::Px(8.0).into());
                    }
                }),
            },
            Case {
                name: "centred in 250pt",
                content: "Centred text that wraps onto a second line",
                runs: None,
                style: rules(|r| {
                    r.width = Some(Length::Px(250.0).into());
                    r.text_align = Some(TextAlign::Center);
                }),
            },
            Case {
                name: "styled runs",
                content: "",
                runs: Some(vec![TextRun::styled("Bold", bold_run), TextRun::plain(" and plain runs that wrap.")]),
                style: rules(|_| {}),
            },
        ]
    }

    fn make(backend: &mut IosBackend, case: &Case, selectable: bool) -> IosNode {
        let a11y = AccessibilityProps::default();
        let node = match (&case.runs, selectable) {
            (Some(runs), false) => backend.create_styled_text(runs, &a11y),
            (Some(runs), true) => backend.create_selectable_styled_text(runs, &a11y),
            (None, false) => backend.create_text(case.content, &a11y),
            (None, true) => backend.create_selectable_text(case.content, &a11y),
        };
        backend.apply_style(&node, &case.style);
        node
    }

    fn selected_copy_available(view: &UIView) -> bool {
        let range = NSRange { location: 0, length: 4 };
        let selects: bool = unsafe { msg_send![view, respondsToSelector: sel!(setSelectedRange:)] };
        if !selects {
            return false;
        }
        let _: () = unsafe { msg_send![view, setSelectedRange: range] };
        let nil: *mut AnyObject = std::ptr::null_mut();
        unsafe { msg_send![view, canPerformAction: sel!(copy:), withSender: nil] }
    }

    pub fn run() {
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let mut backend = IosBackend::new(mtm);
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 400.0, height: 2400.0 } };
        let window: Retained<UIWindow> = unsafe { msg_send_id![mtm.alloc::<UIWindow>(), initWithFrame: frame] };
        let _: () = unsafe { msg_send![&window, makeKeyAndVisible] };
        let host: Retained<UIView> = unsafe { msg_send_id![mtm.alloc::<UIView>(), initWithFrame: frame] };
        unsafe { window.addSubview(&host) };
        backend.set_host_root(host.clone());

        let a11y = AccessibilityProps::default();
        let mut root = backend.create_view(&a11y);
        backend.apply_style(&root, &rules(|r| r.width = Some(Length::Px(300.0).into())));
        let cases = cases();
        let mut pairs = Vec::new();
        for case in &cases {
            let mut pair = Vec::new();
            for selectable in [false, true] {
                // A content-sized cell: the text measures to its content.
                let mut cell = backend.create_view(&a11y);
                backend.apply_style(&cell, &rules(|r| r.align_items = Some(AlignItems::FlexStart)));
                let text = make(&mut backend, case, selectable);
                backend.insert(&mut cell, text.clone());
                backend.insert(&mut root, cell);
                pair.push(text);
            }
            pairs.push((case.name, pair.remove(0), pair.remove(0)));
        }
        backend.finish(root);
        backend.run_layout();
        backend.run_layout();
        let _: () = unsafe { msg_send![&window, layoutIfNeeded] };

        let mut failures = 0;
        for (name, label, selectable) in &pairs {
            assert!(matches!(label, IosNode::Label(_)), "{name}: plain text is a label");
            assert!(matches!(selectable, IosNode::SelectableText(_)), "{name}: selectable text is a text view");
            let lf = backend.frame(label).expect("label laid out");
            let sf = backend.frame(selectable).expect("selectable laid out");
            let same_size = (lf.width, lf.height) == (sf.width, sf.height);
            let (lw, lh, lp) = render(label.as_view());
            let (sw, sh, sp) = render(selectable.as_view());
            let differing = if (lw, lh) == (sw, sh) {
                lp.chunks(4).zip(sp.chunks(4)).filter(|(a, b)| a != b).count()
            } else {
                usize::MAX
            };
            let ink = |w: usize, p: &[u8]| {
                let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
                for (i, px) in p.chunks(4).enumerate() {
                    if px[3] > 0 {
                        let (x, y) = (i % w.max(1), i / w.max(1));
                        x0 = x0.min(x); y0 = y0.min(y); x1 = x1.max(x); y1 = y1.max(y);
                    }
                }
                (x0, y0, x1, y1)
            };
            let (a, b) = (ink(lw, &lp), ink(sw, &sp));
            let near = |x: usize, y: usize| x.abs_diff(y) <= 1;
            let same_ink = near(a.0, b.0) && near(a.1, b.1) && near(a.2, b.2) && near(a.3, b.3);
            let ok = same_size && same_ink;
            if !ok {
                failures += 1;
            }
            println!(
                "[selectable_text] {} {name}: label {}x{}, selectable {}x{}, ink {a:?} / {b:?}, differing pixels {}",
                if ok { "PASS" } else { "FAIL" },
                lf.width,
                lf.height,
                sf.width,
                sf.height,
                if differing == usize::MAX { "n/a".into() } else { differing.to_string() },
            );
        }

        // Selection: the text view is selectable, read-only and takes
        // touches; with a range selected UIKit offers Copy. The label can't.
        let (_, label, selectable) = &pairs[0];
        let view = selectable.as_view();
        let is_selectable: bool = unsafe { msg_send![view, isSelectable] };
        let is_editable: bool = unsafe { msg_send![view, isEditable] };
        let interactive: bool = unsafe { msg_send![view, isUserInteractionEnabled] };
        assert!(is_selectable && !is_editable && interactive, "selectable, read-only, interactive");
        assert!(selected_copy_available(view), "a selected range offers Copy");
        assert!(!selected_copy_available(label.as_view()), "a label offers no selection");

        // A text update reaches the selectable text.
        backend.update_text(selectable, "Updated text");
        let shown: Retained<objc2_foundation::NSString> = unsafe { msg_send_id![view, text] };
        assert_eq!(shown.to_string(), "Updated text", "update_text reaches the text view");

        assert_eq!(failures, 0, "{failures} case(s) differ between the label and the selectable text");
        println!("selectable_text: ok");
    }
}
