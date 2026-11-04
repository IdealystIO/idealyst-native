//! `border_radius` rounds a view's own box and `overflow` clips its children,
//! against real Core Animation pixels. AppKit's backing layer turns `masksToBounds`
//! on whenever `cornerRadius` goes non-zero, so a rounded view clipped its children
//! with no `overflow` on macOS and not on web or iOS (`backend_apple_core::clip`).
//!
//! Pixels come from `-[CALayer renderInContext:]`, which honours `masksToBounds`,
//! corner radii and shadows. `cacheDisplayInRect:` doesn't capture a layer tree. Every
//! sample has a mirror across the box's centre, so the y-flip between the bitmap and
//! the flipped view doesn't matter.
//!
//! `harness = false`: AppKit views and windows are main-thread only.
//!
//! ```sh
//! cargo test -p backend-macos --test rounded_clip
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    use std::rc::Rc;

    use backend_macos::{MacosBackend, MacosNode};
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{msg_send, msg_send_id, Encode, Encoding, RefEncode};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker};
    use runtime_scene::Host as _;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::animation::AnimProp;
    use runtime_shared::{Color, Gradient, GradientKind, GradientStop, Length, Overflow, Position, Shadow, StyleRules, Tokenized};
    use runtime_vocabulary::caps::{AnimationOps, LifecycleOps, StyleOps, ViewOps};

    const W: usize = 300;
    const H: usize = 300;
    /// The rounded box sits here in the window, `BOX` square.
    const AT: usize = 100;
    const BOX: usize = 100;
    const RADIUS: f32 = 30.0;
    /// The child reaches this far past the box on every side.
    const PAST: f32 = 20.0;

    /// Inside the box's bounds, in the corner the radius cuts away.
    const CORNER: (usize, usize) = (AT + 2, AT + 2);
    /// Outside the box's bounds, over the child's overhang.
    const OUTSIDE: (usize, usize) = (AT - 10, AT + BOX / 2);
    /// The middle of the box.
    const MIDDLE: (usize, usize) = (AT + BOX / 2, AT + BOX / 2);

    #[repr(transparent)]
    #[derive(Clone, Copy)]
    struct CGContextRef(*mut c_void);
    unsafe impl Encode for CGContextRef {
        const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("CGContext", &[]));
    }
    unsafe impl RefEncode for CGContextRef {
        const ENCODING_REF: Encoding = Encoding::Pointer(&<Self as Encode>::ENCODING);
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
        fn CGColorSpaceRelease(cs: *mut c_void);
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: *mut c_void,
            bitmap_info: u32,
        ) -> CGContextRef;
        fn CGBitmapContextGetData(ctx: CGContextRef) -> *mut u8;
        fn CGContextRelease(ctx: CGContextRef);
    }

    /// `kCGImageAlphaPremultipliedLast`: an RGBA8 buffer that indexes directly.
    const ALPHA_PREMULTIPLIED_LAST: u32 = 1;

    fn render(root: &NSView) -> Vec<u8> {
        let layer: Retained<NSObject> = unsafe { msg_send_id![root, layer] };
        unsafe {
            let cs = CGColorSpaceCreateDeviceRGB();
            let ctx = CGBitmapContextCreate(std::ptr::null_mut(), W, H, 8, W * 4, cs, ALPHA_PREMULTIPLIED_LAST);
            assert!(!ctx.0.is_null(), "no bitmap context");
            let _: () = msg_send![&layer, renderInContext: ctx];
            let out = std::slice::from_raw_parts(CGBitmapContextGetData(ctx), W * H * 4).to_vec();
            CGContextRelease(ctx);
            CGColorSpaceRelease(cs);
            out
        }
    }

    fn pixel(buf: &[u8], (x, y): (usize, usize)) -> [u8; 4] {
        let i = (y * W + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn is_child(p: [u8; 4]) -> bool {
        p[3] > 200 && p[0] > 200 && p[1] < 60 && p[2] < 60
    }

    fn is_empty(p: [u8; 4]) -> bool {
        p[3] < 10
    }

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    fn px(v: f32) -> Option<Tokenized<Length>> {
        Some(Length::Px(v).into())
    }

    fn radius_all(s: &mut StyleRules, r: f32) {
        s.border_top_left_radius = px(r);
        s.border_top_right_radius = px(r);
        s.border_bottom_left_radius = px(r);
        s.border_bottom_right_radius = px(r);
    }

    fn boxed(f: impl FnOnce(&mut StyleRules)) -> Rc<StyleRules> {
        let mut s = StyleRules {
            position: Some(Position::Absolute),
            left: px(AT as f32),
            top: px(AT as f32),
            width: px(BOX as f32),
            height: px(BOX as f32),
            ..Default::default()
        };
        radius_all(&mut s, RADIUS);
        f(&mut s);
        Rc::new(s)
    }

    struct Scene {
        window: Retained<NSWindow>,
        root: Retained<NSView>,
        parent: MacosNode,
    }

    /// A window with a layer-backed content view for the backend to mount into.
    fn window(mtm: MainThreadMarker, backend: &mut MacosBackend) -> (Retained<NSWindow>, Retained<NSView>) {
        let frame = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: W as f64, height: H as f64 } };
        let window: Retained<NSWindow> = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc::<NSWindow>(),
                frame,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::NSBackingStoreBuffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        let content: Retained<NSView> = unsafe { msg_send_id![mtm.alloc::<NSView>(), initWithFrame: frame] };
        let _: () = unsafe { msg_send![&content, setWantsLayer: true] };
        window.setContentView(Some(&content));
        backend.set_host_root(content.clone());
        (window, content)
    }

    /// A rounded parent and a red child overhanging it on every side.
    fn scene(mtm: MainThreadMarker, backend: &mut MacosBackend, style: &Rc<StyleRules>) -> Scene {
        let (window, content) = window(mtm, backend);
        let a11y = AccessibilityProps::default();
        let mut root = backend.create_view(&a11y);
        backend.apply_style(&root, &Rc::new(StyleRules {
            width: px(W as f32),
            height: px(H as f32),
            ..Default::default()
        }));
        let parent = backend.create_view(&a11y);
        backend.apply_style(&parent, style);
        let mut parent_mut = parent.clone();
        let child = backend.create_view(&a11y);
        backend.apply_style(&child, &Rc::new(StyleRules {
            position: Some(Position::Absolute),
            left: px(-PAST),
            top: px(-PAST),
            width: px(BOX as f32 + 2.0 * PAST),
            height: px(BOX as f32 + 2.0 * PAST),
            background: Some(Tokenized::Literal(Color("#ff0000".into()))),
            ..Default::default()
        }));
        backend.insert(&mut parent_mut, child);
        backend.insert(&mut root, parent_mut);
        backend.finish(root);
        let s = Scene { window, root: content, parent };
        // AppKit attaches a subview's layer to its parent's when the window displays:
        // before that `renderInContext:` on the content view draws nothing.
        s.window.orderFront(None);
        settle(&s, backend);
        s
    }

    /// The same run a live window does: another layout pass, a display, a commit.
    fn settle(s: &Scene, backend: &mut MacosBackend) {
        backend.run_layout_pass_now();
        unsafe { s.window.displayIfNeeded() };
        let _: () = unsafe { msg_send![objc2::class!(CATransaction), flush] };
    }

    fn close(s: &Scene) {
        s.window.orderOut(None);
    }

    /// The bug: a Card with a radius and a child reaching past its corner cut the
    /// child on macOS, where web and iOS draw it.
    fn regression_macos_rounded_view_clips_children_without_overflow(mtm: MainThreadMarker, b: &mut MacosBackend) {
        let s = scene(mtm, b, &boxed(|_| {}));
        let check = |s: &Scene, when: &str| {
            let buf = render(&s.root);
            assert!(is_child(pixel(&buf, MIDDLE)), "{when}: the child draws, got {:?}", pixel(&buf, MIDDLE));
            assert!(is_child(pixel(&buf, CORNER)), "{when}: past the corner too, got {:?}", pixel(&buf, CORNER));
            assert!(is_child(pixel(&buf, OUTSIDE)), "{when}: and past the bounds, got {:?}", pixel(&buf, OUTSIDE));
        };
        check(&s, "in a displayed window");

        // A restyle at the same radius, and another at a new one.
        b.apply_style(&s.parent, &boxed(|_| {}));
        settle(&s, b);
        check(&s, "after a restyle");
        b.apply_style(&s.parent, &boxed(|st| radius_all(st, 10.0)));
        settle(&s, b);
        check(&s, "after a new radius");

        // Through zero and back, which is the transition that sets the flag.
        b.apply_style(&s.parent, &boxed(|st| radius_all(st, 0.0)));
        b.apply_style(&s.parent, &boxed(|_| {}));
        settle(&s, b);
        check(&s, "back from no radius");

        // A radius the layout pass clamps: no px width, so it lands in `sync_corner_radius`.
        b.apply_style(&s.parent, &boxed(|st| {
            st.width = None;
            st.right = px((W - AT - BOX) as f32);
            radius_all(st, 999.0);
        }));
        settle(&s, b);
        check(&s, "a radius clamped by layout");

        // A resize: the layout pass writes frames and the radius again.
        b.apply_style(&s.parent, &boxed(|st| {
            st.width = px(BOX as f32 + 20.0);
        }));
        settle(&s, b);
        check(&s, "after a resize");

        // Re-parented, which is when AppKit rebuilds a subtree's layer attachment.
        let parent_view = view_of(&s.parent);
        let holder: Retained<NSView> = unsafe { msg_send_id![&parent_view, superview] };
        let _: () = unsafe { msg_send![&parent_view, removeFromSuperview] };
        let _: () = unsafe { msg_send![&holder, addSubview: &*parent_view] };
        settle(&s, b);
        check(&s, "after re-parenting");

        // An animated view rebuilding its transform.
        b.apply_style(&s.parent, &boxed(|_| {}));
        b.set_animated_f32(&s.parent, AnimProp::Opacity, 1.0);
        b.set_animated_f32(&s.parent, AnimProp::Scale, 1.0);
        settle(&s, b);
        check(&s, "with an animation on it");
        close(&s);
    }

    /// `overflow: Hidden` still clips, and to the rounded shape, not the bounds.
    fn overflow_hidden_clips_to_the_rounded_shape(mtm: MainThreadMarker, b: &mut MacosBackend) {
        let s = scene(mtm, b, &boxed(|st| st.overflow = Some(Overflow::Hidden)));
        let check = |s: &Scene, when: &str| {
            let buf = render(&s.root);
            assert!(is_child(pixel(&buf, MIDDLE)), "{when}: the child draws inside, got {:?}", pixel(&buf, MIDDLE));
            assert!(is_empty(pixel(&buf, CORNER)), "{when}: cut by the corner, got {:?}", pixel(&buf, CORNER));
            assert!(is_empty(pixel(&buf, OUTSIDE)), "{when}: cut at the bounds, got {:?}", pixel(&buf, OUTSIDE));
        };
        check(&s, "in a displayed window");
        b.apply_style(&s.parent, &boxed(|st| {
            st.overflow = Some(Overflow::Hidden);
            radius_all(st, 40.0);
        }));
        settle(&s, b);
        check(&s, "after a new radius");

        // Dropping `overflow` lets go with the radius still there.
        b.apply_style(&s.parent, &boxed(|_| {}));
        settle(&s, b);
        let buf = render(&s.root);
        assert!(is_child(pixel(&buf, CORNER)), "dropped overflow draws the corner, got {:?}", pixel(&buf, CORNER));
        assert!(is_child(pixel(&buf, OUTSIDE)), "and past the bounds, got {:?}", pixel(&buf, OUTSIDE));
        close(&s);
    }

    fn shadowed(st: &mut StyleRules) {
        st.background = Some(Tokenized::Literal(Color("#ffffff".into())));
        st.shadow = Some(Shadow {
            x: 0.0,
            y: 0.0,
            blur: 40.0,
            color: Color("#000000".into()).into(),
        });
    }

    /// With no child in the way, how much shadow reaches past the box.
    fn shadow_alpha(mtm: MainThreadMarker, b: &mut MacosBackend, style: &Rc<StyleRules>) -> (u8, u8) {
        let (window, content) = window(mtm, b);
        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);
        b.apply_style(&root, &Rc::new(StyleRules { width: px(W as f32), height: px(H as f32), ..Default::default() }));
        let parent = b.create_view(&a11y);
        b.apply_style(&parent, style);
        let parent_node = parent.clone();
        b.insert(&mut root, parent);
        b.finish(root);
        b.run_layout_pass_now();
        window.orderFront(None);
        unsafe { window.displayIfNeeded() };
        let _: () = unsafe { msg_send![objc2::class!(CATransaction), flush] };
        let mounted = pixel(&render(&content), OUTSIDE)[3];
        // A sibling shadow needs the view's layer in its parent's, which AppKit does at
        // the display after the mount's layout pass, so it joins at the next sync.
        b.apply_style(&parent_node, style);
        let restyled = pixel(&render(&content), OUTSIDE)[3];
        window.orderOut(None);
        (mounted, restyled)
    }

    /// A rounded view's own shadow paints outside its bounds, which the accidental
    /// clip cut away, and `overflow: Hidden` keeps it through the sibling layer.
    fn regression_macos_rounded_view_lost_its_shadow(mtm: MainThreadMarker, b: &mut MacosBackend) {
        let plain = shadow_alpha(mtm, b, &boxed(shadowed));
        assert!(plain.0 > 20, "a rounded view's shadow paints at mount, alpha {}", plain.0);
        assert!(plain.1 > 20, "and after a restyle, alpha {}", plain.1);
        let square = shadow_alpha(mtm, b, &boxed(|st| {
            shadowed(st);
            radius_all(st, 0.0);
        }));
        assert!(square.0 > 20, "and a square one's, which `addSubview:` zeroed at mount, alpha {}", square.0);
        let clipped = shadow_alpha(mtm, b, &boxed(|st| {
            shadowed(st);
            st.overflow = Some(Overflow::Hidden);
        }));
        assert!(clipped.1 > 20, "with overflow: Hidden too, on the sibling, alpha {}", clipped.1);
    }

    /// A gradient is the box's own background, a sublayer no mask shapes any more. It
    /// follows the radius, at mount (deferred until layout) and through a restyle.
    fn a_gradient_follows_the_radius_without_a_mask(mtm: MainThreadMarker, b: &mut MacosBackend) {
        let gradient = |st: &mut StyleRules| {
            st.background_gradient = Some(Gradient {
                kind: GradientKind::Linear { angle_deg: 90.0 },
                stops: vec![
                    GradientStop { offset: 0.0, color: Tokenized::Literal(Color("#0000ff".into())) },
                    GradientStop { offset: 1.0, color: Tokenized::Literal(Color("#0000ff".into())) },
                ],
            });
        };
        let (window, content) = window(mtm, b);
        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);
        b.apply_style(&root, &Rc::new(StyleRules { width: px(W as f32), height: px(H as f32), ..Default::default() }));
        let parent = b.create_view(&a11y);
        // No px width: the radius waits for the layout pass.
        b.apply_style(&parent, &boxed(|st| {
            gradient(st);
            st.width = None;
            st.right = px((W - AT - BOX) as f32);
        }));
        b.insert(&mut root, parent.clone());
        b.finish(root);
        b.run_layout_pass_now();
        window.orderFront(None);
        unsafe { window.displayIfNeeded() };
        let _: () = unsafe { msg_send![objc2::class!(CATransaction), flush] };
        let buf = render(&content);
        assert!(pixel(&buf, MIDDLE)[3] > 200, "the gradient paints, got {:?}", pixel(&buf, MIDDLE));
        assert!(is_empty(pixel(&buf, CORNER)), "rounded after layout, got {:?}", pixel(&buf, CORNER));

        b.apply_style(&parent, &boxed(gradient));
        let buf = render(&content);
        assert!(is_empty(pixel(&buf, CORNER)), "after a restyle, got {:?}", pixel(&buf, CORNER));

        b.apply_style(&parent, &boxed(|st| {
            gradient(st);
            radius_all(st, 0.0);
        }));
        let buf = render(&content);
        assert!(pixel(&buf, CORNER)[3] > 200, "square with no radius, got {:?}", pixel(&buf, CORNER));
        window.orderOut(None);
    }

    /// A field paints a square fill no `cornerRadius` reaches and has no children, so
    /// it keeps the mask AppKit gives it: that is what rounds it.
    fn a_rounded_input_still_rounds_what_it_paints(mtm: MainThreadMarker, b: &mut MacosBackend) {
        use runtime_vocabulary::caps::TextInputOps;
        let (window, content) = window(mtm, b);
        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);
        b.apply_style(&root, &Rc::new(StyleRules { width: px(W as f32), height: px(H as f32), ..Default::default() }));
        let input = b.create_text_input("", None, Rc::new(|_| {}), None, None, false, &a11y);
        b.apply_style(&input, &boxed(|st| st.background = Some(Tokenized::Literal(Color("#ff0000".into())))));
        b.insert(&mut root, input);
        b.finish(root);
        b.run_layout_pass_now();
        window.orderFront(None);
        unsafe { window.displayIfNeeded() };
        let _: () = unsafe { msg_send![objc2::class!(CATransaction), flush] };
        let buf = render(&content);
        assert!(is_child(pixel(&buf, MIDDLE)), "the field fills, got {:?}", pixel(&buf, MIDDLE));
        assert!(is_empty(pixel(&buf, CORNER)), "rounded, got {:?}", pixel(&buf, CORNER));
        window.orderOut(None);
    }

    /// A view that clips with no style keeps its own clip when it is rounded.
    fn an_image_keeps_its_own_clip_when_rounded(b: &mut MacosBackend) {
        use runtime_vocabulary::caps::ImageOps;
        let node = b.create_image("", None, &AccessibilityProps::default());
        let layer: Retained<NSObject> = unsafe { msg_send_id![&*view_of(&node), layer] };
        let masks = |l: &NSObject| -> bool { unsafe { msg_send![l, masksToBounds] } };
        assert!(masks(&layer), "an image view pins its clip so a cover image crops");
        b.apply_style(&node, &boxed(|_| {}));
        assert!(masks(&layer), "a radius leaves it");
        b.apply_style(&node, &boxed(|st| radius_all(st, 0.0)));
        assert!(masks(&layer), "and so does dropping one");
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(unsafe { pthread_main_np() } != 0, "rounded_clip must run on the main thread (harness = false)");
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
        check("regression_macos_rounded_view_clips_children_without_overflow", &mut || regression_macos_rounded_view_clips_children_without_overflow(mtm, &mut backend));
        check("overflow_hidden_clips_to_the_rounded_shape", &mut || overflow_hidden_clips_to_the_rounded_shape(mtm, &mut backend));
        check("regression_macos_rounded_view_lost_its_shadow", &mut || regression_macos_rounded_view_lost_its_shadow(mtm, &mut backend));
        check("a_gradient_follows_the_radius_without_a_mask", &mut || a_gradient_follows_the_radius_without_a_mask(mtm, &mut backend));
        check("a_rounded_input_still_rounds_what_it_paints", &mut || a_rounded_input_still_rounds_what_it_paints(mtm, &mut backend));
        check("an_image_keeps_its_own_clip_when_rounded", &mut || an_image_keeps_its_own_clip_when_rounded(&mut backend));
        assert_eq!(failed, 0, "{failed} rounded_clip case(s) failed");
    }
}
