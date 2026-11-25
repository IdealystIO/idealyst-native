//! Percent translates (`Transform::TranslateX(Length::Percent(_))`) against
//! real AppKit: they resolve, against the view's laid-out size, into the
//! layer transform (not the frame), compose with px translates, animate
//! under `transitions { transform }`, land in the first commit that shows
//! the view, and are hit-tested and measured where they paint.
//!
//! The app shape is a sliding card: `[TranslateX(Percent(100)),
//! TranslateX(Px(-PEEK))]` ↔ `[TranslateX(Percent(0)), TranslateX(Px(0))]`
//! with `transitions { transform: 200ms EaseOut }`, and a child that
//! counter-slides with `TranslateX(Percent(-100))` + px. Web slides it; macOS
//! put the percent part in the frame origin, which the layout pass writes
//! with implicit actions off, so it snapped.
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
                COMMITTED.with(|c| c.set(Some(layer_translate(&view))));
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
        let content: Retained<NSView> =
            unsafe { msg_send_id![mtm.alloc::<NSView>(), initWithFrame: rect] };
        window.setContentView(Some(&content));
        let backend = Rc::new(RefCell::new(MacosBackend::new(mtm)));
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
    /// percent + px translate under `transitions { transform }`. The percent
    /// part sat in the frame origin, so the frame said x = 300 and the layer
    /// only −40, and the change to `Percent(0)` animated the px part alone
    /// while the frame jumped. The frame must be the layout box, the layer
    /// must carry 300 − 40, and the change must animate the layer from there.
    fn regression_percent_translate_change_animates(m: &Mounted) {
        let card = create_styled(m, &boxed(0.0, 0.0, CARD_W, CARD_H, card_hidden(), true));
        insert_and_layout(m, &m.root, &card);
        spin(0.05);
        let v = view_of(&card);

        let f = frame_of(&v);
        assert!(close(f.origin.x, 0.0), "the frame is the layout box alone, got x = {}", f.origin.x);
        let t = model_transform(&v);
        let hidden_x = (CARD_W - PEEK) as f64;
        assert!(close(t.m41, hidden_x), "the layer carries 100% − PEEK = {hidden_x}, got {}", t.m41);

        restyle_and_layout(m, &card, &boxed(0.0, 0.0, CARD_W, CARD_H, card_shown(), true));

        let (key_path, from) = transition_animation(&v)
            .expect("a percent translate change under `transitions { transform }` animates");
        assert_eq!(key_path, "transform");
        assert!(close(from.m41, hidden_x), "the slide starts where the card painted, got {}", from.m41);
        let t = model_transform(&v);
        assert!(close(t.m41, 0.0), "the model transform is the target, got m41 = {}", t.m41);
        assert!(close(frame_of(&v).origin.x, 0.0), "the frame stays the layout box");
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
    /// drag-and-drop, the robot's click point) must sum both translates;
    /// `convertRect:` sees neither.
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

    /// A click goes to the view where it paints: at `translateX(100%)` the
    /// card covers x in [300, 600), not its layout box [0, 300).
    fn percent_translated_view_is_hit_where_it_paints(m: &Mounted) {
        let pct = vec![Transform::TranslateX(Length::Percent(100.0))];
        let card = create_styled(m, &boxed(0.0, 0.0, CARD_W, CARD_H, pct, false));
        insert_and_layout(m, &m.root, &card);
        spin(0.05);
        let v = view_of(&card);
        let root_v = view_of(&m.root);
        let hit = |x: f64, y: f64| -> Option<Retained<NSView>> {
            let pt = CGPoint { x, y };
            let in_content: CGPoint =
                unsafe { msg_send![&*m.content, convertPoint: pt, fromView: &*root_v] };
            let in_super: CGPoint = match unsafe { m.content.superview() } {
                Some(sup) => unsafe {
                    msg_send![&sup, convertPoint: in_content, fromView: &*m.content]
                },
                None => in_content,
            };
            unsafe { msg_send_id![&*m.content, hitTest: in_super] }
        };
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

    /// A static `translate(-50%, -50%)` (idea-ui's centering) with no
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
        assert!(
            close(got.0, -(W as f64) / 2.0) && close(got.1, -(H as f64) / 2.0),
            "the first commit carries translate(-50%, -50%) = ({}, {}), got {got:?}",
            -W / 2.0,
            -H / 2.0
        );
        let node = watched.borrow_mut().take().expect("mounted");
        let f = frame_of(&view_of(&node));
        assert!(
            close(f.origin.x, 300.0) && close(f.origin.y, 200.0),
            "the frame is the layout box, got {:?}",
            f.origin
        );
        remove(m, &node);
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
        println!("percent_translate_transition: 6 passed");
    }
}
