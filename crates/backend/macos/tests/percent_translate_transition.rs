//! Percent translates (`Transform::TranslateX(Length::Percent(_))`) against
//! real AppKit: they resolve against the view's laid-out size into the
//! frame, where AppKit's own geometry (control tracking, hover, cursors)
//! sees them; compose with px translates; animate in the layer under
//! `transitions { transform }`; are in place in the first commit that shows
//! the view; and are hit-tested and measured where they paint.
//!
//! The app shape is a sliding card: `[TranslateX(Percent(100)),
//! TranslateX(Px(-PEEK))]` ↔ `[TranslateX(Percent(0)), TranslateX(Px(0))]`
//! with `transitions { transform: 200ms EaseOut }`, and a child that
//! counter-slides with `TranslateX(Percent(-100))` + px. Web slides it. The
//! frame moves at once on macOS, so the layer has to play the way there.
//!
//! The windows are never ordered on screen. `harness = false`: AppKit views
//! and windows are main-thread only, and libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test percent_translate_transition
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;
    use std::rc::Rc;

    use backend_apple_core::cg::CATransform3D;
    use backend_macos::{MacosBackend, MacosNode};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSString};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::{Easing, Length, Position, StyleRules, Tokenized, Transform, Transition};
    use runtime_vocabulary::caps::{IntrospectionOps, LifecycleOps, StyleOps, ViewOps};

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFRunLoopCommonModes: *const c_void;
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopGetMain() -> *mut c_void;
        fn CFRunLoopObserverCreate(
            allocator: *const c_void,
            activities: usize,
            repeats: u8,
            order: isize,
            callout: extern "C" fn(*mut c_void, usize, *mut c_void),
            context: *mut c_void,
        ) -> *mut c_void;
        fn CFRunLoopAddObserver(rl: *mut c_void, observer: *mut c_void, mode: *const c_void);
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: u8) -> i32;
    }

    /// `kCFRunLoopBeforeWaiting`.
    const BEFORE_WAITING: usize = 1 << 5;
    /// Above Core Animation's commit observer, 2_000_000.
    const AFTER_COMMIT_ORDER: isize = 2_100_000;

    const WINDOW_W: f64 = 600.0;
    const WINDOW_H: f64 = 400.0;
    const CARD_W: f32 = 300.0;
    const CARD_H: f32 = 200.0;
    const PEEK: f32 = 40.0;
    /// `apply_static_transform`'s animation key.
    const TRANSITION_KEY: &str = "idealyst-transform-transition";

    thread_local! {
        static WATCHED: RefCell<Option<Retained<NSView>>> = const { RefCell::new(None) };
        static COMMITTED: Cell<Option<(f64, f64)>> = const { Cell::new(None) };
    }

    fn layer_of(view: &NSView) -> *mut AnyObject {
        let layer: *mut AnyObject = unsafe { msg_send![view, layer] };
        assert!(!layer.is_null(), "a transformed view is layer-backed");
        layer
    }

    fn model_transform(view: &NSView) -> CATransform3D {
        unsafe { msg_send![layer_of(view), transform] }
    }

    fn layer_translate(view: &NSView) -> (f64, f64) {
        let layer: *mut AnyObject = unsafe { msg_send![view, layer] };
        if layer.is_null() {
            return (0.0, 0.0);
        }
        let t: CATransform3D = unsafe { msg_send![layer, transform] };
        (t.m41, t.m42)
    }

    /// Where `view` paints in its superview: its frame moved by its layer.
    fn painted_at(view: &NSView) -> (f64, f64) {
        let (f, t) = (frame_of(view), layer_translate(view));
        (f.origin.x + t.0, f.origin.y + t.1)
    }

    fn frame_of(view: &NSView) -> CGRect {
        unsafe { msg_send![view, frame] }
    }

    /// The transition animation on `view`'s layer: its key path and the
    /// matrix it starts from. `None` when the layer has no such animation.
    fn transition_animation(view: &NSView) -> Option<(String, CATransform3D)> {
        let key = NSString::from_str(TRANSITION_KEY);
        let anim: *mut AnyObject = unsafe { msg_send![layer_of(view), animationForKey: &*key] };
        if anim.is_null() {
            return None;
        }
        let key_path: Option<Retained<NSString>> = unsafe { msg_send_id![anim, keyPath] };
        let from: *mut AnyObject = unsafe { msg_send![anim, fromValue] };
        assert!(!from.is_null(), "the transition starts from the on-screen matrix");
        let from: CATransform3D = unsafe { msg_send![from, CATransform3DValue] };
        Some((key_path.map(|k| k.to_string()).unwrap_or_default(), from))
    }

    extern "C" fn after_commit(_o: *mut c_void, _a: usize, _i: *mut c_void) {
        WATCHED.with(|w| {
            if let Some(view) = w.borrow_mut().take() {
                COMMITTED.with(|c| c.set(Some(painted_at(&view))));
            }
        });
    }

    fn install_commit_probe() {
        unsafe {
            let observer = CFRunLoopObserverCreate(
                std::ptr::null(),
                BEFORE_WAITING,
                1,
                AFTER_COMMIT_ORDER,
                after_commit,
                std::ptr::null_mut(),
            );
            CFRunLoopAddObserver(CFRunLoopGetMain(), observer, kCFRunLoopCommonModes);
        }
    }

    fn spin(seconds: f64) {
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, 0) };
    }

    fn view_of(node: &MacosNode) -> Retained<NSView> {
        match node {
            MacosNode::View(v) => v.clone(),
            MacosNode::Label(l) => Retained::into_super(Retained::into_super(l.clone())),
        }
    }

    fn px(v: f32) -> Option<Tokenized<Length>> {
        Some(Tokenized::Literal(Length::Px(v)))
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.01
    }

    /// An absolutely-placed `w`×`h` box at (`left`, `top`) with `transform`,
    /// and `transitions { transform: 200ms EaseOut }` when `transition`.
    fn boxed(
        left: f32,
        top: f32,
        w: f32,
        h: f32,
        transform: Vec<Transform>,
        transition: bool,
    ) -> Rc<StyleRules> {
        Rc::new(StyleRules {
            position: Some(Position::Absolute),
            top: px(top),
            left: px(left),
            width: px(w),
            height: px(h),
            transform: Some(transform),
            transform_transition: transition.then(|| Transition::new(200, Easing::EaseOut)),
            ..Default::default()
        })
    }

    fn card_hidden() -> Vec<Transform> {
        vec![Transform::TranslateX(Length::Percent(100.0)), Transform::TranslateX(Length::Px(-PEEK))]
    }

    fn card_shown() -> Vec<Transform> {
        vec![Transform::TranslateX(Length::Percent(0.0)), Transform::TranslateX(Length::Px(0.0))]
    }

    struct Mounted {
        backend: Rc<RefCell<MacosBackend>>,
        root: MacosNode,
        content: Retained<NSView>,
        _window: Retained<NSWindow>,
    }

    fn mount(mtm: MainThreadMarker) -> Mounted {
        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: WINDOW_W, height: WINDOW_H },
        };
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
        let backend = Rc::new(RefCell::new(MacosBackend::new(mtm)));
        // The backend's own (flipped) host root, so window rects are top-left.
        let content = backend.borrow().create_host_root().expect("host root");
        let bounds = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: rect.size };
        let _: () = unsafe { msg_send![&content, setFrame: bounds] };
        window.setContentView(Some(&content));
        backend_macos::install_global_self(Rc::downgrade(&backend));
        let root = {
            let mut b = backend.borrow_mut();
            b.set_host_root(content.clone());
            let root = b.create_view(&AccessibilityProps::default());
            b.apply_style(
                &root,
                &Rc::new(StyleRules {
                    width: px(WINDOW_W as f32),
                    height: px(WINDOW_H as f32),
                    ..Default::default()
                }),
            );
            b.finish(root.clone());
            b.run_layout_pass_now();
            root
        };
        spin(0.05);
        Mounted { backend, root, content, _window: window }
    }

    /// Insert `child` under `parent` and lay out.
    fn insert_and_layout(m: &Mounted, parent: &MacosNode, child: &MacosNode) {
        let mut b = m.backend.borrow_mut();
        let mut parent = parent.clone();
        b.insert(&mut parent, child.clone());
        b.run_layout_pass_now();
    }

    fn create_styled(m: &Mounted, style: &Rc<StyleRules>) -> MacosNode {
        let mut b = m.backend.borrow_mut();
        let node = b.create_view(&AccessibilityProps::default());
        b.apply_style(&node, style);
        node
    }

    fn restyle(m: &Mounted, node: &MacosNode, style: &Rc<StyleRules>) {
        m.backend.borrow_mut().apply_style(node, style);
    }

    fn restyle_and_layout(m: &Mounted, node: &MacosNode, style: &Rc<StyleRules>) {
        let mut b = m.backend.borrow_mut();
        b.apply_style(node, style);
        b.run_layout_pass_now();
    }

    fn remove(m: &Mounted, node: &MacosNode) {
        let mut b = m.backend.borrow_mut();
        b.remove_child(&m.root, node);
        b.run_layout_pass_now();
    }

    /// REGRESSION (the Timely content card snapped on macOS, slid on web): a
    /// percent + px translate under `transitions { transform }`. Only the px
    /// part is the layer's, so the change to `Percent(0)` animated that while
    /// the frame jumped. The layer must start where the card painted, seen
    /// from where the frame now is.
    fn regression_percent_translate_change_animates(m: &Mounted) {
        let card = create_styled(m, &boxed(0.0, 0.0, CARD_W, CARD_H, card_hidden(), true));
        insert_and_layout(m, &m.root, &card);
        spin(0.05);
        let v = view_of(&card);

        let f = frame_of(&v);
        assert!(close(f.origin.x, CARD_W as f64), "the frame holds the percent, got x = {}", f.origin.x);
        let t = model_transform(&v);
        assert!(close(t.m41, -PEEK as f64), "the layer holds the px, got {}", t.m41);

        // No layout pass: a change of transform is not a change of layout.
        restyle(m, &card, &boxed(0.0, 0.0, CARD_W, CARD_H, card_shown(), true));
        let shown = |what: &str| {
            let (key_path, from) = transition_animation(&v)
                .expect("a percent translate change under `transitions { transform }` animates");
            assert_eq!(key_path, "transform");
            let hidden_x = (CARD_W - PEEK) as f64;
            assert!(close(from.m41, hidden_x), "{what}: the slide starts where the card painted, got {}", from.m41);
            let t = model_transform(&v);
            assert!(close(t.m41, 0.0), "{what}: the model transform is the target, got m41 = {}", t.m41);
            assert!(close(frame_of(&v).origin.x, 0.0), "{what}: the frame is at the target");
        };
        shown("after the restyle");
        m.backend.borrow_mut().run_layout_pass_now();
        shown("after a layout pass");
        remove(m, &card);
    }

    /// A slide in flight stays where it is on screen when its card is
    /// reversed, and when a layout pass resizes the card under it.
    fn a_slide_in_flight_survives_a_reversal_and_a_resize(m: &Mounted) {
        // The frame's x plus the matrix's is where the card starts from.
        let pct = |p: f32| vec![Transform::TranslateX(Length::Percent(p))];
        let card = create_styled(m, &boxed(0.0, 0.0, CARD_W, CARD_H, pct(100.0), true));
        insert_and_layout(m, &m.root, &card);
        spin(0.05);
        let v = view_of(&card);
        restyle(m, &card, &boxed(0.0, 0.0, CARD_W, CARD_H, pct(0.0), true));
        // Resting at 50% is the frame moving under the animation.
        restyle(m, &card, &boxed(0.0, 0.0, CARD_W, CARD_H, pct(50.0), true));
        let (_, from) = transition_animation(&v).expect("retargeted");
        let painted = frame_of(&v).origin.x + from.m41;
        assert!(close(painted, CARD_W as f64), "still starts at 100%, got {painted}");
        // Twice as wide: 50% is now CARD_W, and the start has not moved.
        restyle_and_layout(m, &card, &boxed(0.0, 0.0, CARD_W * 2.0, CARD_H, pct(50.0), true));
        assert!(close(frame_of(&v).origin.x, CARD_W as f64), "50% of twice the width");
        let (_, from) = transition_animation(&v).expect("still in flight");
        let painted = frame_of(&v).origin.x + from.m41;
        assert!(close(painted, CARD_W as f64), "the start holds across the resize, got {painted}");
        assert!(close(model_transform(&v).m41, 0.0), "at rest the layer holds nothing");

        // Reversed after some frames were drawn: it turns around where it is.
        spin(0.05);
        let drawn = {
            let layer: *mut AnyObject = unsafe { msg_send![layer_of(&v), presentationLayer] };
            let t: CATransform3D = unsafe { msg_send![layer, transform] };
            let p: CGPoint = unsafe { msg_send![layer, position] };
            p.x + t.m41
        };
        restyle(m, &card, &boxed(0.0, 0.0, CARD_W * 2.0, CARD_H, pct(-100.0), true));
        let (_, from) = transition_animation(&v).expect("reversed");
        let painted = frame_of(&v).origin.x + from.m41;
        // The slide ran on for the instant between the two reads.
        assert!((painted - drawn).abs() < 5.0, "turns around at {drawn}, got {painted}");
        remove(m, &card);
    }

    /// A style that drops its percent transform goes back to its layout box,
    /// with or without a layout pass after it.
    fn dropping_the_transform_restores_the_layout_box(m: &Mounted) {
        let pct = vec![Transform::TranslateX(Length::Percent(100.0))];
        let card = create_styled(m, &boxed(20.0, 0.0, CARD_W, CARD_H, pct, false));
        insert_and_layout(m, &m.root, &card);
        let v = view_of(&card);
        assert!(close(frame_of(&v).origin.x, 20.0 + CARD_W as f64));
        restyle(m, &card, &boxed(20.0, 0.0, CARD_W, CARD_H, Vec::new(), false));
        assert!(close(frame_of(&v).origin.x, 20.0), "at once");
        m.backend.borrow_mut().run_layout_pass_now();
        assert!(close(frame_of(&v).origin.x, 20.0), "and after a pass");
        assert!(close(model_transform(&v).m41, 0.0));
        remove(m, &card);
    }

    /// Inside a scaled ancestor a view is found where it paints: its offset
    /// from the ancestor's centre scales with it.
    fn regression_a_child_of_a_scaled_view_reports_its_painted_origin(m: &Mounted) {
        let card = create_styled(
            m,
            &boxed(0.0, 0.0, 200.0, 100.0, vec![Transform::Scale(0.5)], false),
        );
        // Flush with the card's right edge: paints at 100 + 0.5 * (200 - 100).
        let inner = create_styled(m, &boxed(200.0, 100.0, 20.0, 20.0, Vec::new(), false));
        {
            let mut b = m.backend.borrow_mut();
            let mut parent = card.clone();
            b.insert(&mut parent, inner.clone());
        }
        insert_and_layout(m, &m.root, &card);
        let r = m.backend.borrow().absolute_frame(&inner).expect("mounted in a window");
        assert!(
            (r.x - 150.0).abs() < 0.01 && (r.y - 75.0).abs() < 0.01,
            "the card's corner paints at (150, 75), got ({}, {})",
            r.x,
            r.y
        );
        remove(m, &card);
    }

    /// A change carried by the percent entry alone (`Percent(100)` →
    /// `Percent(0)`, no px entry) animates too; it used to compare equal.
    fn regression_percent_only_change_animates(m: &Mounted) {
        let pct = |p: f32| vec![Transform::TranslateX(Length::Percent(p))];
        let card = create_styled(m, &boxed(0.0, 0.0, CARD_W, CARD_H, pct(100.0), true));
        insert_and_layout(m, &m.root, &card);
        spin(0.05);
        let v = view_of(&card);
        restyle_and_layout(m, &card, &boxed(0.0, 0.0, CARD_W, CARD_H, pct(0.0), true));
        let (key_path, from) =
            transition_animation(&v).expect("a percent-only change animates");
        assert_eq!(key_path, "transform");
        assert!(close(from.m41, CARD_W as f64), "starts at 100% = {CARD_W}, got {}", from.m41);
        assert!(close(model_transform(&v).m41, 0.0));
        remove(m, &card);
    }

    /// Without a transition the same change snaps: no animation, model at
    /// the target.
    fn percent_change_without_transition_snaps(m: &Mounted) {
        let card = create_styled(m, &boxed(0.0, 0.0, CARD_W, CARD_H, card_hidden(), false));
        insert_and_layout(m, &m.root, &card);
        spin(0.05);
        let v = view_of(&card);
        restyle_and_layout(m, &card, &boxed(0.0, 0.0, CARD_W, CARD_H, card_shown(), false));
        assert!(transition_animation(&v).is_none(), "no transition declared → no animation");
        assert!(close(model_transform(&v).m41, 0.0));
        remove(m, &card);
    }

    /// The card's child counter-slides with `Percent(-100)` + PEEK, so it
    /// paints at the card's layout origin. Its window rect (overlay anchoring,
    /// drag-and-drop, the robot's click point) must count both translates;
    /// `convertRect:` sees only the percent parts.
    fn regression_counter_slid_child_reports_its_painted_rect(m: &Mounted) {
        let card = create_styled(m, &boxed(0.0, 0.0, CARD_W, CARD_H, card_hidden(), true));
        let inner = create_styled(
            m,
            &boxed(
                0.0,
                0.0,
                CARD_W,
                CARD_H,
                vec![
                    Transform::TranslateX(Length::Percent(-100.0)),
                    Transform::TranslateX(Length::Px(PEEK)),
                ],
                true,
            ),
        );
        {
            let mut b = m.backend.borrow_mut();
            let mut card_parent = card.clone();
            b.insert(&mut card_parent, inner.clone());
        }
        insert_and_layout(m, &m.root, &card);
        spin(0.05);
        let r = m.backend.borrow().absolute_frame(&inner).expect("mounted in a window");
        assert!(
            r.x.abs() < 0.01 && r.y.abs() < 0.01,
            "card (+{}) and inner (−{}) cancel, so inner paints at (0, 0); got ({}, {})",
            CARD_W - PEEK,
            CARD_W - PEEK,
            r.x,
            r.y
        );
        let r = m.backend.borrow().absolute_frame(&card).expect("mounted in a window");
        assert!((r.x - (CARD_W - PEEK)).abs() < 0.01, "the card paints at 100% − PEEK, got {}", r.x);
        remove(m, &card);
    }

    /// What a click at root `(x, y)` lands on.
    fn hit(m: &Mounted, x: f64, y: f64) -> Option<Retained<NSView>> {
        let pt = CGPoint { x, y };
        let root_v = view_of(&m.root);
        let in_content: CGPoint =
            unsafe { msg_send![&*m.content, convertPoint: pt, fromView: &*root_v] };
        let in_super: CGPoint = match unsafe { m.content.superview() } {
            Some(sup) => unsafe {
                msg_send![&sup, convertPoint: in_content, fromView: &*m.content]
            },
            None => in_content,
        };
        unsafe { msg_send_id![&*m.content, hitTest: in_super] }
    }

    fn hits(m: &Mounted, x: f64, y: f64, node: &MacosNode) -> bool {
        hit(m, x, y).is_some_and(|h| std::ptr::eq(&*h, &*view_of(node)))
    }

    /// REGRESSION (Timely's calendar button, turned 180°, couldn't be
    /// pressed): the hit test undid only the translate of the layer's matrix.
    /// Turning in place is translate · rotate · translate back, so a 36pt
    /// button turned 180° has a (36, 36) translate and its clicks landed 36pt
    /// off. Its rect was off by as much.
    fn regression_a_rotated_or_scaled_view_is_hit_where_it_paints(m: &Mounted) {
        let check = |what: &str, t: Vec<Transform>, inside: &[(f64, f64)], outside: &[(f64, f64)], rect: [f32; 4]| {
            // A 40×20 box centred on (120, 110).
            let v = create_styled(m, &boxed(100.0, 100.0, 40.0, 20.0, t, false));
            insert_and_layout(m, &m.root, &v);
            for &(x, y) in inside {
                assert!(hits(m, x, y, &v), "{what}: ({x}, {y}) is on it, got {:?}", hit(m, x, y));
            }
            for &(x, y) in outside {
                assert!(!hits(m, x, y, &v), "{what}: ({x}, {y}) is off it");
            }
            let r = m.backend.borrow().absolute_frame(&v).expect("in a window");
            let got = [r.x, r.y, r.width, r.height];
            assert!(
                got.iter().zip(rect).all(|(a, b)| (a - b).abs() < 0.01),
                "{what}: paints at {rect:?}, reports {got:?}"
            );
            remove(m, &v);
        };
        check(
            "turned 180°",
            vec![Transform::Rotate(180.0)],
            &[(102.0, 102.0), (138.0, 118.0), (120.0, 110.0)],
            &[(98.0, 110.0), (142.0, 110.0), (142.0, 122.0)],
            [100.0, 100.0, 40.0, 20.0],
        );
        check(
            "turned 90°",
            vec![Transform::Rotate(90.0)],
            &[(112.0, 92.0), (128.0, 128.0)],
            &[(102.0, 110.0), (138.0, 110.0)],
            [110.0, 90.0, 20.0, 40.0],
        );
        check(
            "halved",
            vec![Transform::Scale(0.5)],
            &[(111.0, 106.0), (129.0, 114.0)],
            &[(105.0, 110.0), (120.0, 103.0)],
            [110.0, 105.0, 20.0, 10.0],
        );
        check("collapsed", vec![Transform::Scale(0.0)], &[], &[(120.0, 110.0)], [120.0, 110.0, 0.0, 0.0]);
    }

    /// A child moved out of its parent by its transform alone takes clicks
    /// where it paints, as one laid out past the edge does.
    fn a_child_translated_out_of_its_parent_takes_the_click(m: &Mounted) {
        let card = create_styled(m, &boxed(100.0, 100.0, 40.0, 40.0, Vec::new(), false));
        let t = vec![Transform::TranslateX(Length::Px(-60.0))];
        let tab = create_styled(m, &boxed(0.0, 0.0, 40.0, 40.0, t, false));
        {
            let mut b = m.backend.borrow_mut();
            let mut parent = card.clone();
            b.insert(&mut parent, tab.clone());
        }
        insert_and_layout(m, &m.root, &card);
        assert!(hits(m, 50.0, 120.0, &tab), "got {:?}", hit(m, 50.0, 120.0));
        assert!(!hits(m, 120.0, 120.0, &tab), "not where it was laid out");
        remove(m, &card);
    }

    /// A click goes to the view where it paints: at `translateX(100%)` the
    /// card covers x in [300, 600), not its layout box [0, 300).
    fn percent_translated_view_is_hit_where_it_paints(m: &Mounted) {
        let pct = vec![Transform::TranslateX(Length::Percent(100.0))];
        let card = create_styled(m, &boxed(0.0, 0.0, CARD_W, CARD_H, pct, false));
        insert_and_layout(m, &m.root, &card);
        spin(0.05);
        let v = view_of(&card);
        let hit = |x: f64, y: f64| hit(m, x, y);
        let painted = hit(CARD_W as f64 + 50.0, 100.0);
        assert!(
            painted.as_deref().is_some_and(|h| std::ptr::eq(h, &*v)),
            "a click where the card paints hits it; got {painted:?}"
        );
        let layout_box = hit(50.0, 100.0);
        assert!(
            !layout_box.as_deref().is_some_and(|h| std::ptr::eq(h, &*v)),
            "a click on the card's untranslated box misses it; got {layout_box:?}"
        );
        remove(m, &card);
    }

    /// A static `translate(-50%, -50%)` (centring a box on a point) with no
    /// transition is in place in the Core Animation commit that first shows
    /// the view, though AppKit builds the layer into the tree in that commit.
    /// The view mounts the way the scene mounts it: off the main queue.
    fn regression_static_percent_translate_lands_in_first_commit(m: &Mounted) {
        const W: f32 = 100.0;
        const H: f32 = 60.0;
        COMMITTED.with(|c| c.set(None));
        let watched: Rc<RefCell<Option<MacosNode>>> = Rc::new(RefCell::new(None));
        let (backend, root, slot) = (m.backend.clone(), m.root.clone(), watched.clone());
        runtime_shared::schedule_microtask(move || {
            let centred = {
                let mut b = backend.borrow_mut();
                let node = b.create_view(&AccessibilityProps::default());
                b.apply_style(
                    &node,
                    &boxed(
                        300.0,
                        200.0,
                        W,
                        H,
                        vec![
                            Transform::TranslateX(Length::Percent(-50.0)),
                            Transform::TranslateY(Length::Percent(-50.0)),
                        ],
                        false,
                    ),
                );
                let mut root = root;
                b.insert(&mut root, node.clone());
                node
            };
            WATCHED.with(|w| *w.borrow_mut() = Some(view_of(&centred)));
            *slot.borrow_mut() = Some(centred);
        });
        let mut waited = 0;
        while COMMITTED.with(|c| c.get()).is_none() && waited < 100 {
            spin(0.01);
            waited += 1;
        }
        let got = COMMITTED.with(|c| c.get()).expect("the commit probe fired");
        let want = (300.0 - W as f64 / 2.0, 200.0 - H as f64 / 2.0);
        assert!(
            close(got.0, want.0) && close(got.1, want.1),
            "the first commit paints the box centred at {want:?}, got {got:?}"
        );
        let node = watched.borrow_mut().take().expect("mounted");
        let f = frame_of(&view_of(&node));
        assert!(
            close(f.origin.x, want.0) && close(f.origin.y, want.1),
            "the frame holds the percent, got {:?}",
            f.origin
        );
        remove(m, &node);
    }

    /// REGRESSION: a native control in a box centred with
    /// `translate(-50%, -50%)` works where it paints. AppKit's own geometry
    /// (an NSButton's tracking, hover tracking areas, cursor rects) walks
    /// frames and ignores layer transforms, so with the percent in the layer
    /// the button paints in one place and tracks the mouse in another.
    fn regression_a_control_in_a_percent_centred_box_takes_the_click(m: &Mounted, transition: bool) {
        use runtime_vocabulary::caps::ButtonOps;
        const W: f32 = 200.0;
        const H: f32 = 100.0;
        let centre = vec![
            Transform::TranslateX(Length::Percent(-50.0)),
            Transform::TranslateY(Length::Percent(-50.0)),
        ];
        let boxed_node = create_styled(m, &boxed(300.0, 200.0, W, H, centre, transition));
        let clicks = Rc::new(Cell::new(0u32));
        let button = {
            let clicks = clicks.clone();
            let action = runtime_shared::Action {
                method: "",
                inputs: Vec::new(),
                initial: Vec::new(),
                output: None,
                fire: Rc::new(move || clicks.set(clicks.get() + 1)),
            };
            let mut b = m.backend.borrow_mut();
            let button =
                b.create_button("Go", &action, None, None, &AccessibilityProps::default());
            b.apply_style(&button, &boxed(0.0, 0.0, 80.0, 30.0, Vec::new(), false));
            let mut parent = boxed_node.clone();
            b.insert(&mut parent, button.clone());
            button
        };
        insert_and_layout(m, &m.root, &boxed_node);
        spin(0.05);

        // Where AppKit's frame chain puts the box is where it paints.
        let v = view_of(&boxed_node);
        let bounds: CGRect = unsafe { msg_send![&v, bounds] };
        let tracked: CGRect = unsafe { msg_send![&v, convertRect: bounds, toView: &*m.content] };
        assert!(
            close(tracked.origin.x, 300.0 - W as f64 / 2.0)
                && close(tracked.origin.y, 200.0 - H as f64 / 2.0),
            "hover and cursor rects sit on the painted box, got {:?}",
            tracked.origin
        );

        // A real click at the button's painted centre.
        let r = m.backend.borrow().absolute_frame(&button).expect("mounted in a window");
        let at = CGPoint { x: (r.x + r.width / 2.0) as f64, y: (r.y + r.height / 2.0) as f64 };
        let in_window: CGPoint =
            unsafe { msg_send![&m.content, convertPoint: at, toView: std::ptr::null::<NSView>()] };
        click(m, in_window);
        assert_eq!(clicks.get(), 1, "the button fires where it paints");
        remove(m, &boxed_node);
    }

    /// Mouse down at `at` (window space), with the up already queued for the
    /// control's tracking loop.
    fn click(m: &Mounted, at: CGPoint) {
        use objc2::class;
        const DOWN: usize = 1;
        const UP: usize = 2;
        let app: Retained<AnyObject> =
            unsafe { msg_send_id![class!(NSApplication), sharedApplication] };
        let number: isize = unsafe { msg_send![&m._window, windowNumber] };
        let event = |kind: usize| -> Retained<AnyObject> {
            unsafe {
                msg_send_id![
                    class!(NSEvent),
                    mouseEventWithType: kind,
                    location: at,
                    modifierFlags: 0usize,
                    timestamp: 0.0f64,
                    windowNumber: number,
                    context: std::ptr::null_mut::<AnyObject>(),
                    eventNumber: 0isize,
                    clickCount: 1isize,
                    pressure: 1.0f32,
                ]
            }
        };
        let (down, up) = (event(DOWN), event(UP));
        let in_super: CGPoint = unsafe {
            match m.content.superview() {
                Some(sup) => {
                    msg_send![&sup, convertPoint: at, fromView: std::ptr::null::<NSView>()]
                }
                None => at,
            }
        };
        let hit: Option<Retained<NSView>> =
            unsafe { msg_send_id![&*m.content, hitTest: in_super] };
        let hit = hit.expect("the click lands on a view");
        unsafe {
            let _: () = msg_send![&*app, postEvent: &*up, atStart: false];
            let _: () = msg_send![&hit, mouseDown: &*down];
        }
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        install_commit_probe();
        let m = mount(mtm);
        regression_percent_translate_change_animates(&m);
        regression_percent_only_change_animates(&m);
        percent_change_without_transition_snaps(&m);
        regression_counter_slid_child_reports_its_painted_rect(&m);
        percent_translated_view_is_hit_where_it_paints(&m);
        regression_static_percent_translate_lands_in_first_commit(&m);
        regression_a_rotated_or_scaled_view_is_hit_where_it_paints(&m);
        a_child_translated_out_of_its_parent_takes_the_click(&m);
        a_slide_in_flight_survives_a_reversal_and_a_resize(&m);
        dropping_the_transform_restores_the_layout_box(&m);
        regression_a_child_of_a_scaled_view_reports_its_painted_origin(&m);
        regression_a_control_in_a_percent_centred_box_takes_the_click(&m, false);
        regression_a_control_in_a_percent_centred_box_takes_the_click(&m, true);
        println!("percent_translate_transition: 13 passed");
    }
}
