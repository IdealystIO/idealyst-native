//! A style that drops a prop against real AppKit: the view goes back to what it
//! had with no style, the way a class swap resets it on web, and an animation on
//! the same field is left alone.
//!
//! Colour and font size are also read from a rendered bitmap, because a property
//! read only proves what was written.
//!
//! `harness = false` for the same reason as `text_align`: AppKit views and windows
//! are main-thread only, and libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test style_reset
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
    use objc2_app_kit::{NSBackingStoreType, NSTextView, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker, NSObject, NSString};
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::animation::AnimProp;
    use runtime_shared::{
        Color, Cursor, FontWeight, Gradient, GradientKind, GradientStop, Length, ObjectFit, Overflow,
        PointerEvents, PresenceState, StyleRules, Tokenized, Transform,
    };
    use runtime_vocabulary::caps::{AnimationOps, ImageOps, PresenceOps, StyleOps, TextInputOps, TextOps, ViewOps};

    const WIDTH: f64 = 400.0;
    const HEIGHT: f64 = 120.0;

    fn rect() -> CGRect {
        CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: WIDTH, height: HEIGHT } }
    }

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    fn color(hex: &str) -> Option<Tokenized<Color>> {
        Some(Tokenized::Literal(Color(hex.into())))
    }

    fn rules(f: impl FnOnce(&mut StyleRules)) -> Rc<StyleRules> {
        let mut s = StyleRules::default();
        f(&mut s);
        Rc::new(s)
    }

    fn none() -> Rc<StyleRules> {
        Rc::new(StyleRules::default())
    }

    fn alpha(view: &NSView) -> f64 {
        unsafe { msg_send![view, alphaValue] }
    }

    fn font_size(view: &NSView) -> f64 {
        let font: Retained<NSObject> = unsafe { msg_send_id![view, font] };
        unsafe { msg_send![&font, pointSize] }
    }

    fn font_weight_trait(view: &NSView) -> u64 {
        const NS_FONT_BOLD_TRAIT: u64 = 1 << 1;
        let font: Retained<NSObject> = unsafe { msg_send_id![view, font] };
        let descriptor: Retained<NSObject> = unsafe { msg_send_id![&font, fontDescriptor] };
        let traits: u32 = unsafe { msg_send![&descriptor, symbolicTraits] };
        traits as u64 & NS_FONT_BOLD_TRAIT
    }

    /// sRGB red and blue of a `textColor`.
    fn text_rb(view: &NSView) -> (f64, f64) {
        let c: Retained<NSObject> = unsafe { msg_send_id![view, textColor] };
        let space: Retained<NSObject> = unsafe { msg_send_id![objc2::class!(NSColorSpace), sRGBColorSpace] };
        let c: Retained<NSObject> = unsafe { msg_send_id![&c, colorUsingColorSpace: &*space] };
        unsafe { (msg_send![&c, redComponent], msg_send![&c, blueComponent]) }
    }

    /// The most saturated colour `view` drew, and how many rows hold ink.
    fn ink(view: &NSView) -> ((f64, f64, f64), usize) {
        let bounds: CGRect = unsafe { msg_send![view, bounds] };
        let rep: Retained<NSObject> = unsafe { msg_send_id![view, bitmapImageRepForCachingDisplayInRect: bounds] };
        let _: () = unsafe { msg_send![view, cacheDisplayInRect: bounds, toBitmapImageRep: &*rep] };
        let w: isize = unsafe { msg_send![&rep, pixelsWide] };
        let h: isize = unsafe { msg_send![&rep, pixelsHigh] };
        let scale = h as f64 / bounds.size.height;
        let mut rows = 0;
        let mut best = ((0.0, 0.0, 0.0), -1.0);
        for y in 0..h {
            let mut row = false;
            for x in 0..w {
                let c: Option<Retained<NSObject>> = unsafe { msg_send_id![&rep, colorAtX: x, y: y] };
                let Some(c) = c else { continue };
                let (r, g, b, a): (CGFloat, CGFloat, CGFloat, CGFloat) = unsafe {
                    (msg_send![&c, redComponent], msg_send![&c, greenComponent], msg_send![&c, blueComponent], msg_send![&c, alphaComponent])
                };
                if a < 0.5 {
                    continue;
                }
                row = true;
                let spread = r.max(g).max(b) - r.min(g).min(b);
                if spread > best.1 {
                    best = ((r, g, b), spread);
                }
            }
            rows += row as usize;
        }
        (best.0, (rows as f64 / scale) as usize)
    }

    fn set_frame(view: &NSView) {
        let _: () = unsafe { msg_send![view, setFrame: rect()] };
    }

    fn layer(view: &NSView) -> Retained<NSObject> {
        let _: () = unsafe { msg_send![view, setWantsLayer: true] };
        unsafe { msg_send_id![view, layer] }
    }

    fn find_text_view(view: &NSView) -> Option<Retained<NSView>> {
        let is_text_view: bool = unsafe { msg_send![view, isKindOfClass: NSTextView::class()] };
        if is_text_view {
            return unsafe { Retained::retain(view as *const NSView as *mut NSView) };
        }
        unsafe { view.subviews() }.iter().find_map(|sub| find_text_view(sub))
    }

    fn big_red_bold() -> Rc<StyleRules> {
        rules(|s| {
            s.color = color("#ff0000");
            s.font_size = Some(Tokenized::Literal(Length::Px(40.0)));
            s.font_weight = Some(FontWeight::Bold);
            s.opacity = Some(Tokenized::Literal(0.5));
        })
    }

    /// The reported shape: a label on a variant with a colour, a size and `opacity`
    /// flipping to one without them stayed big, red and half-transparent.
    fn regression_macos_dropped_props_kept_on_a_label(backend: &mut MacosBackend) {
        let node = backend.create_text("Twelve", &AccessibilityProps::default());
        let view = view_of(&node);
        set_frame(&view);
        let natural_size = font_size(&view);
        let natural_color = text_rb(&view);
        let (_, natural_rows) = ink(&view);

        backend.apply_style(&node, &big_red_bold());
        assert_eq!(font_size(&view), 40.0);
        assert_eq!(alpha(&view), 0.5);
        let ((r, _, b), rows) = ink(&view);
        assert!(r > 0.8 && b < 0.2, "the label draws red, got {r} {b}");
        assert!(rows > natural_rows, "and taller, {rows} rows against {natural_rows}");

        backend.apply_style(&node, &none());
        assert_eq!(font_size(&view), natural_size, "font size");
        assert_eq!(font_weight_trait(&view), 0, "font weight");
        assert_eq!(alpha(&view), 1.0, "opacity");
        assert_eq!(text_rb(&view), natural_color, "colour");
        let ((r, _, _), rows) = ink(&view);
        assert!(r < 0.5, "the bitmap follows the colour, got {r}");
        assert_eq!(rows, natural_rows, "and the size");
    }

    /// A field's and a text area's unstyled font and colour differ from a label's,
    /// so they go back to what THEY had.
    fn regression_macos_dropped_props_kept_on_inputs(backend: &mut MacosBackend) {
        let input = backend.create_text_input("Twelve", None, Rc::new(|_| {}), None, None, false, &AccessibilityProps::default());
        let area = backend.create_text_area("One", None, true, None, None, Rc::new(|_| {}), None, &AccessibilityProps::default());
        let input_view = view_of(&input);
        let area_view = view_of(&area);
        let inner = find_text_view(&area_view).expect("a text area holds an NSTextView");
        backend.apply_style(&input, &none());
        backend.apply_style(&area, &none());
        let natural = (font_size(&input_view), font_size(&inner), text_rb(&input_view), text_rb(&inner));

        backend.apply_style(&input, &big_red_bold());
        backend.apply_style(&area, &big_red_bold());
        assert_eq!(font_size(&input_view), 40.0);
        assert_eq!(font_size(&inner), 40.0);

        backend.apply_style(&input, &none());
        backend.apply_style(&area, &none());
        assert_eq!(font_size(&input_view), natural.0, "input font");
        assert_eq!(font_size(&inner), natural.1, "text area font");
        assert_eq!(text_rb(&input_view), natural.2, "input colour");
        assert_eq!(text_rb(&inner), natural.3, "text area colour");
        assert_eq!(alpha(&input_view), 1.0, "input opacity");
        assert_eq!(alpha(&area_view), 1.0, "text area opacity");
    }

    /// The box props of a view, next to opacity in the same path.
    fn regression_macos_dropped_props_kept_on_a_view(backend: &mut MacosBackend) {
        let node = backend.create_view(&AccessibilityProps::default());
        let view = view_of(&node);
        set_frame(&view);
        let l = layer(&view);
        let radius = |l: &NSObject| -> f64 { unsafe { msg_send![l, cornerRadius] } };
        let masks = |l: &NSObject| -> bool { unsafe { msg_send![l, masksToBounds] } };
        let border = |l: &NSObject| -> f64 { unsafe { msg_send![l, borderWidth] } };
        let natural_masks = masks(&l);
        let gradient = |l: &NSObject| -> usize {
            let subs: Option<Retained<objc2_foundation::NSArray<NSObject>>> = unsafe { msg_send_id![l, sublayers] };
            subs.map(|s| {
                s.iter()
                    .filter(|sub| {
                        let name: Option<Retained<NSString>> = unsafe { msg_send_id![*sub, name] };
                        name.is_some_and(|n| n.to_string() == "idealyst_gradient")
                    })
                    .count()
            })
            .unwrap_or(0)
        };

        backend.apply_style(&node, &rules(|s| {
            s.opacity = Some(Tokenized::Literal(0.5));
            s.overflow = Some(Overflow::Hidden);
            s.border_top_left_radius = Some(Tokenized::Literal(Length::Px(12.0)));
            s.border_top_width = Some(Tokenized::Literal(2.0));
            s.border_right_width = Some(Tokenized::Literal(2.0));
            s.border_bottom_width = Some(Tokenized::Literal(2.0));
            s.border_left_width = Some(Tokenized::Literal(2.0));
            s.border_top_color = color("#00ff00");
            s.border_right_color = color("#00ff00");
            s.border_bottom_color = color("#00ff00");
            s.border_left_color = color("#00ff00");
            s.background = color("#0000ff");
            s.background_gradient = Some(Gradient {
                kind: GradientKind::Linear { angle_deg: 90.0 },
                stops: vec![
                    GradientStop { offset: 0.0, color: Tokenized::Literal(Color("#ff0000".into())) },
                    GradientStop { offset: 1.0, color: Tokenized::Literal(Color("#0000ff".into())) },
                ],
            });
            s.transform = Some(vec![Transform::Scale(2.0)]);
            s.cursor = Some(Cursor::Pointer);
            s.pointer_events = Some(PointerEvents::None);
        }));
        assert_eq!(radius(&l), 12.0);
        assert!(masks(&l));
        assert_eq!(gradient(&l), 1);
        assert!(view_at_center_is_transparent(&view), "pointer_events: None passes the click through");

        backend.apply_style(&node, &none());
        assert_eq!(alpha(&view), 1.0, "opacity");
        assert_eq!(radius(&l), 0.0, "corner radius");
        assert_eq!(masks(&l), natural_masks, "overflow");
        assert_eq!(border(&l), 0.0, "border");
        assert_eq!(gradient(&l), 0, "gradient");
        let bg: CGColorRef = unsafe { msg_send![&l, backgroundColor] };
        let alpha_of_bg = if bg.0.is_null() { 0.0 } else { unsafe { CGColorGetAlpha(bg.0) } };
        assert_eq!(alpha_of_bg, 0.0, "background");
        let m34: backend_apple_core::cg::CATransform3D = unsafe { msg_send![&l, transform] };
        assert_eq!(m34.m11, 1.0, "transform");
        assert!(!view_at_center_is_transparent(&view), "pointer_events");
    }

    #[repr(transparent)]
    struct CGColorRef(*const std::ffi::c_void);
    unsafe impl objc2::Encode for CGColorRef {
        const ENCODING: objc2::Encoding =
            objc2::Encoding::Pointer(&objc2::Encoding::Struct("CGColor", &[]));
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGColorGetAlpha(color: *const std::ffi::c_void) -> CGFloat;
    }

    fn view_at_center_is_transparent(view: &NSView) -> bool {
        let p = CGPoint { x: 10.0, y: 10.0 };
        let hit: *mut NSView = unsafe { msg_send![view, hitTest: p] };
        hit.is_null()
    }

    /// A layer that clips with no style keeps clipping when `overflow` is dropped.
    fn dropped_overflow_keeps_an_images_own_clip(backend: &mut MacosBackend) {
        let node = backend.create_image("", None, &AccessibilityProps::default());
        let view = view_of(&node);
        let l = layer(&view);
        let natural: bool = unsafe { msg_send![&l, masksToBounds] };
        assert!(natural, "an image view pins its clip so a cover image crops");
        backend.apply_style(&node, &rules(|s| s.overflow = Some(Overflow::Visible)));
        backend.apply_style(&node, &none());
        let clip: bool = unsafe { msg_send![&l, masksToBounds] };
        assert_eq!(clip, natural);
    }

    fn dropped_object_fit_goes_back_to_contain(backend: &mut MacosBackend) {
        let node = backend.create_image("", None, &AccessibilityProps::default());
        let l = layer(&view_of(&node));
        let gravity = |l: &NSObject| -> String {
            let g: Retained<NSString> = unsafe { msg_send_id![l, contentsGravity] };
            g.to_string()
        };
        backend.apply_style(&node, &none());
        let contain = gravity(&l);
        backend.apply_style(&node, &rules(|s| s.object_fit = Some(ObjectFit::Cover)));
        assert_ne!(gravity(&l), contain);
        backend.apply_style(&node, &none());
        assert_eq!(gravity(&l), contain);
    }

    /// `alphaValue` is one field for static and animated opacity, so a restyle,
    /// stated or dropped, must not snap an `AnimatedValue` to the stylesheet.
    fn restyle_leaves_an_animated_opacity_alone(backend: &mut MacosBackend) {
        let node = backend.create_view(&AccessibilityProps::default());
        let view = view_of(&node);
        backend.apply_style(&node, &rules(|s| s.opacity = Some(Tokenized::Literal(0.5))));
        backend.set_animated_f32(&node, AnimProp::Opacity, 0.25);
        backend.apply_style(&node, &none());
        assert_eq!(alpha(&view), 0.25, "a drop doesn't touch it");
        backend.apply_style(&node, &rules(|s| s.opacity = Some(Tokenized::Literal(0.75))));
        assert_eq!(alpha(&view), 0.25, "nor does a stated value");
    }

    /// Same for a colour an animation drives.
    fn restyle_leaves_an_animated_text_color_alone(backend: &mut MacosBackend) {
        let node = backend.create_text("Twelve", &AccessibilityProps::default());
        let view = view_of(&node);
        backend.apply_style(&node, &none());
        backend.set_animated_color(&node, AnimProp::ForegroundColor, [0.0, 0.0, 1.0, 1.0]);
        backend.apply_style(&node, &none());
        let (_, b) = text_rb(&view);
        assert!(b > 0.9, "the animated colour stays, got blue {b}");
    }

    /// A transform is held by its static slots, separate from the animated ones.
    fn dropped_transform_keeps_the_animated_one(backend: &mut MacosBackend) {
        let node = backend.create_view(&AccessibilityProps::default());
        let l = layer(&view_of(&node));
        backend.apply_style(&node, &rules(|s| s.transform = Some(vec![Transform::Scale(2.0)])));
        backend.set_animated_f32(&node, AnimProp::Scale, 0.5);
        backend.apply_style(&node, &none());
        let m: backend_apple_core::cg::CATransform3D = unsafe { msg_send![&l, transform] };
        assert_eq!(m.m11, 0.5, "the static 2.0 is gone, the animated 0.5 stays");
    }

    /// A presence stating an opacity holds the field through a restyle; at rest it
    /// lets go onto the static value, not onto a flat 1.0.
    fn presence_holds_opacity_and_rests_on_the_static_value(backend: &mut MacosBackend) {
        let node = backend.create_view(&AccessibilityProps::default());
        let view = view_of(&node);
        backend.apply_style(&node, &rules(|s| s.opacity = Some(Tokenized::Literal(0.5))));
        backend.apply_presence(&node, PresenceState::default().opacity(0.0), None);
        assert_eq!(alpha(&view), 0.0);
        backend.apply_style(&node, &rules(|s| s.opacity = Some(Tokenized::Literal(0.5))));
        assert_eq!(alpha(&view), 0.0, "a restyle during the enter doesn't show the view");
        backend.apply_presence(&node, PresenceState::rest(), None);
        assert_eq!(alpha(&view), 0.5, "at rest it is the stylesheet's");
        backend.apply_style(&node, &none());
        assert_eq!(alpha(&view), 1.0, "and a drop reaches it again");
    }

    /// A focus-ring restyle reaches the font and colour paths. Writing them must not
    /// cost the field its edit, the bug `setAlignment:` had.
    fn dropped_font_keeps_a_focused_edit(mtm: MainThreadMarker, backend: &mut MacosBackend) {
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
        backend.apply_style(&node, &big_red_bold());
        assert!(window.makeFirstResponder(Some(&view)), "the field takes focus");
        let editing = || -> bool {
            let editor: Option<Retained<NSObject>> = unsafe { msg_send_id![&view, currentEditor] };
            editor.is_some()
        };
        assert!(editing(), "editing");
        backend.apply_style(&node, &big_red_bold());
        assert!(editing(), "a restyle with the same font keeps the edit");
        backend.apply_style(&node, &none());
        assert!(editing(), "so does a drop");
        assert!(window.makeFirstResponder(None));
        window.orderOut(None);
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(unsafe { pthread_main_np() } != 0, "style_reset must run on the main thread (harness = false)");
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
        check("regression_macos_dropped_props_kept_on_a_label", &mut || regression_macos_dropped_props_kept_on_a_label(&mut backend));
        check("regression_macos_dropped_props_kept_on_inputs", &mut || regression_macos_dropped_props_kept_on_inputs(&mut backend));
        check("regression_macos_dropped_props_kept_on_a_view", &mut || regression_macos_dropped_props_kept_on_a_view(&mut backend));
        check("dropped_overflow_keeps_an_images_own_clip", &mut || dropped_overflow_keeps_an_images_own_clip(&mut backend));
        check("dropped_object_fit_goes_back_to_contain", &mut || dropped_object_fit_goes_back_to_contain(&mut backend));
        check("restyle_leaves_an_animated_opacity_alone", &mut || restyle_leaves_an_animated_opacity_alone(&mut backend));
        check("restyle_leaves_an_animated_text_color_alone", &mut || restyle_leaves_an_animated_text_color_alone(&mut backend));
        check("dropped_transform_keeps_the_animated_one", &mut || dropped_transform_keeps_the_animated_one(&mut backend));
        check("presence_holds_opacity_and_rests_on_the_static_value", &mut || presence_holds_opacity_and_rests_on_the_static_value(&mut backend));
        check("dropped_font_keeps_a_focused_edit", &mut || dropped_font_keeps_a_focused_edit(mtm, &mut backend));
        assert_eq!(failed, 0, "{failed} style_reset case(s) failed");
    }
}
