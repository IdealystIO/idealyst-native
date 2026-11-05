//! What a Core Animation commit shows after an update that did not come from
//! an AppKit event, against real AppKit and a real run loop.
//!
//! The probe is a run-loop observer ordered after Core Animation's commit, so
//! it reads the frames that commit carried. Pixels would need a visible window
//! and a vsync to land on; the frames are what decide them.
//!
//! `harness = false` for the same reason as `focus_attach`: AppKit views and
//! windows are main-thread only, and libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test layout_before_commit
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

    use backend_macos::{MacosBackend, MacosNode};
    use objc2::rc::Retained;
    use objc2::{msg_send, msg_send_id};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::primitives::portal::{
        AnchorTarget, AnchorableHandle, ElementAlign, ElementSide, PortalTarget,
    };
    use runtime_shared::{EdgeInsets, Length, Ref, StyleRules, ViewportRect};
    use runtime_vocabulary::caps::{LifecycleOps, PortalOps, StyleOps, ViewOps};

    const WINDOW_W: f64 = 600.0;
    const WINDOW_H: f64 = 400.0;
    const TRIGGER: ViewportRect = ViewportRect { x: 400.0, y: 20.0, width: 40.0, height: 30.0 };

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

    thread_local! {
        /// The view to read after the next commit, and what that read found.
        static WATCHED: RefCell<Option<Retained<NSView>>> = const { RefCell::new(None) };
        static COMMITTED: Cell<Option<CGRect>> = const { Cell::new(None) };
    }

    extern "C" fn after_commit(_o: *mut c_void, _a: usize, _i: *mut c_void) {
        WATCHED.with(|w| {
            if let Some(view) = w.borrow_mut().take() {
                let frame: CGRect = unsafe { msg_send![&view, frame] };
                COMMITTED.with(|c| c.set(Some(frame)));
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

    fn sized(w: f32, h: f32) -> Rc<StyleRules> {
        Rc::new(StyleRules {
            width: Some(Length::Px(w).into()),
            height: Some(Length::Px(h).into()),
            ..Default::default()
        })
    }

    fn frame_of(view: &NSView) -> CGRect {
        unsafe { msg_send![view, frame] }
    }

    #[derive(Clone)]
    struct FixedAnchor;
    impl AnchorableHandle for FixedAnchor {
        fn rect(&self) -> ViewportRect {
            TRIGGER
        }
    }

    struct Mounted {
        backend: Rc<RefCell<MacosBackend>>,
        root: MacosNode,
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
            b.set_host_root(content);
            let root = b.create_view(&AccessibilityProps::default());
            b.finish(root.clone());
            b.run_layout_pass_now();
            root
        };
        Mounted { backend, root, _window: window }
    }

    /// REGRESSION (a row rebuilt by a 1 s engine poll blinked): an update off
    /// a timer or a future has no AppKit event to flush layout after it, so
    /// the only pass was a main-queue block. A run loop that has just drained
    /// the main queue goes to `BeforeWaiting` without draining it again, so
    /// Core Animation committed the new views at zero size, one frame before
    /// the pass. Scheduling from a main-queue block makes that order certain.
    fn regression_an_update_off_the_main_queue_is_laid_out_before_the_commit(m: &Mounted) {
        COMMITTED.with(|c| c.set(None));
        let (backend, root) = (m.backend.clone(), m.root.clone());
        runtime_shared::schedule_microtask(move || {
            let mut b = backend.borrow_mut();
            let child = b.create_view(&AccessibilityProps::default());
            b.apply_style(&child, &sized(80.0, 40.0));
            let view = view_of(&child);
            let mut root = root;
            b.insert(&mut root, child);
            WATCHED.with(|w| *w.borrow_mut() = Some(view));
        });
        let mut waited = 0;
        while COMMITTED.with(|c| c.get()).is_none() && waited < 100 {
            spin(0.01);
            waited += 1;
        }
        let frame = COMMITTED.with(|c| c.get()).expect("the commit probe fired");
        assert!(
            frame.size.width == 80.0 && frame.size.height == 40.0,
            "the commit that first shows a new view must carry its laid-out frame, got {frame:?}"
        );
    }

    /// REGRESSION (an open Menu flickered on every layout pass): the pass
    /// wrote the panel's fallback frame over its placement, and the tracker
    /// only moved it back on its next tick, so the frame between was committed.
    /// The assertions read straight after the pass, before any tick can run.
    fn regression_a_layout_pass_keeps_an_anchored_panel_on_its_trigger(m: &Mounted) {
        let a11y = AccessibilityProps::default();
        let anchor: Ref<FixedAnchor> = Ref::new();
        anchor.fill(FixedAnchor);
        let content = {
            let mut b = m.backend.borrow_mut();
            let mut portal = b.create_portal(
                PortalTarget::Anchor {
                    target: AnchorTarget::from(anchor),
                    side: ElementSide::Below,
                    align: ElementAlign::End,
                    offset: 4.0,
                    collision_insets: EdgeInsets::ZERO,
                },
                None,
                false,
                &a11y,
            );
            let content = b.create_view(&a11y);
            b.apply_style(&content, &sized(120.0, 60.0));
            b.insert(&mut portal, content.clone());
            b.run_layout_pass_now();
            content
        };
        let panel = view_of(&content);
        let right = (TRIGGER.x + TRIGGER.width) as f64;
        let below = (TRIGGER.y + TRIGGER.height + 4.0) as f64;

        // Let the tracker place and reveal it.
        let mut waited = 0;
        while (frame_of(&panel).origin.x - (right - 120.0)).abs() > 0.5 && waited < 100 {
            spin(0.01);
            waited += 1;
        }
        let placed = frame_of(&panel);
        assert!(
            (placed.origin.x - (right - 120.0)).abs() < 0.5 && (placed.origin.y - below).abs() < 0.5,
            "the tracker must place an End-aligned panel under its trigger, got {placed:?}"
        );

        m.backend.borrow_mut().run_layout_pass_now();
        let after_pass = frame_of(&panel);
        assert!(
            (after_pass.origin.x - placed.origin.x).abs() < 0.5
                && (after_pass.origin.y - placed.origin.y).abs() < 0.5,
            "a layout pass must leave a placed panel where it is: {placed:?} became {after_pass:?}"
        );

        // A panel whose content grew stays on the trigger in that same pass:
        // End alignment moves its left edge by the new width.
        {
            let mut b = m.backend.borrow_mut();
            b.apply_style(&content, &sized(200.0, 60.0));
            b.run_layout_pass_now();
        }
        let grown = frame_of(&panel);
        assert!(
            (grown.size.width - 200.0).abs() < 0.5 && (grown.origin.x - (right - 200.0)).abs() < 0.5,
            "a resized panel must be re-placed in the pass that resized it, got {grown:?}"
        );
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        install_commit_probe();
        let m = mount(mtm);
        regression_an_update_off_the_main_queue_is_laid_out_before_the_commit(&m);
        regression_a_layout_pass_keeps_an_anchored_panel_on_its_trigger(&m);
        println!("layout_before_commit: 2 passed");
    }
}
