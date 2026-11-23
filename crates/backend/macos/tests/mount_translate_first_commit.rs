//! What the first Core Animation commit carries for a view given an animated
//! translate as it mounts, against real AppKit and a real run loop.
//!
//! The probe is a run-loop observer ordered after Core Animation's commit (as
//! in `layout_before_commit`), so it reads the layer transform that commit
//! carried. AppKit builds a new subview's layer into the window's layer tree
//! inside that commit, and the build writes the view's own (identity)
//! transform onto the layer, so a transform read straight after the write
//! says nothing about what is shown; only the read after the commit does.
//!
//! `harness = false`: AppKit views and windows are main-thread only, and
//! libtest runs cases on worker threads.
//!
//! ```sh
//! cargo test -p backend-macos --test mount_translate_first_commit
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
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};
    use runtime_scene::Host;
    use runtime_shared::accessibility::AccessibilityProps;
    use runtime_shared::animation::AnimProp;
    use runtime_shared::{Length, Position, StyleRules, Tokenized, Transform};
    use runtime_vocabulary::caps::{LifecycleOps, StyleOps, ViewOps};

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

    const GHOST_X: f32 = 200.0;
    const GHOST_Y: f32 = 100.0;

    thread_local! {
        /// The view to read after the next commit, and the translate that
        /// commit carried for it.
        static WATCHED: RefCell<Option<Retained<NSView>>> = const { RefCell::new(None) };
        static COMMITTED: Cell<Option<(f64, f64)>> = const { Cell::new(None) };
    }

    fn layer_translate(view: &NSView) -> (f64, f64) {
        let layer: *mut AnyObject = unsafe { msg_send![view, layer] };
        if layer.is_null() {
            return (0.0, 0.0);
        }
        let t: CATransform3D = unsafe { msg_send![layer, transform] };
        (t.m41, t.m42)
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

    /// The drag layer's clip: pinned to the window's four edges.
    fn clip_style() -> Rc<StyleRules> {
        Rc::new(StyleRules {
            position: Some(Position::Absolute),
            top: px(0.0),
            right: px(0.0),
            bottom: px(0.0),
            left: px(0.0),
            ..Default::default()
        })
    }

    /// The ghost: out of flow at the clip's top-left, placed by translate.
    fn ghost_style() -> StyleRules {
        StyleRules {
            position: Some(Position::Absolute),
            top: px(0.0),
            left: px(0.0),
            width: Some(Length::Px(50.0).into()),
            height: Some(Length::Px(30.0).into()),
            ..Default::default()
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
            size: CGSize { width: 600.0, height: 400.0 },
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
        // Let AppKit build the root's layer tree, so only the views mounted
        // by each case are new to it.
        spin(0.05);
        Mounted { backend, root, _window: window }
    }

    /// Mount the drag layer (clip + ghost) off the main queue and return
    /// the translate carried by the first commit that shows the ghost.
    /// `placement` says how the translate is given.
    #[derive(Clone, Copy)]
    enum Placement {
        /// `set_animated_f32` before the view is inserted (the scene's order:
        /// a handle is filled before its node is inserted).
        AnimatedBeforeInsert,
        /// `set_animated_f32` after the view is inserted.
        AnimatedAfterInsert,
        /// A static `transform` in the view's style.
        Static,
    }

    fn mount_ghost_and_read_first_commit(m: &Mounted, placement: Placement) -> (f64, f64) {
        COMMITTED.with(|c| c.set(None));
        let (backend, root) = (m.backend.clone(), m.root.clone());
        runtime_shared::schedule_microtask(move || {
            let place = |ghost: &MacosNode| {
                backend_macos::set_animated_f32(ghost, AnimProp::TranslateX, GHOST_X);
                backend_macos::set_animated_f32(ghost, AnimProp::TranslateY, GHOST_Y);
            };
            let (mut clip, ghost) = {
                let mut b = backend.borrow_mut();
                let clip = b.create_view(&AccessibilityProps::default());
                b.apply_style(&clip, &clip_style());
                let ghost = b.create_view(&AccessibilityProps::default());
                let mut style = ghost_style();
                if let Placement::Static = placement {
                    style.transform = Some(vec![
                        Transform::TranslateX(Length::Px(GHOST_X)),
                        Transform::TranslateY(Length::Px(GHOST_Y)),
                    ]);
                }
                b.apply_style(&ghost, &Rc::new(style));
                (clip, ghost)
            };
            if let Placement::AnimatedBeforeInsert = placement {
                place(&ghost);
            }
            {
                let mut b = backend.borrow_mut();
                b.insert(&mut clip, ghost.clone());
                let mut root = root;
                b.insert(&mut root, clip);
            }
            if let Placement::AnimatedAfterInsert = placement {
                place(&ghost);
            }
            WATCHED.with(|w| *w.borrow_mut() = Some(view_of(&ghost)));
        });
        let mut waited = 0;
        while COMMITTED.with(|c| c.get()).is_none() && waited < 100 {
            spin(0.01);
            waited += 1;
        }
        COMMITTED.with(|c| c.get()).expect("the commit probe fired")
    }

    fn assert_placed(got: (f64, f64), case: &str) {
        assert!(
            (got.0 - GHOST_X as f64).abs() < 0.01 && (got.1 - GHOST_Y as f64).abs() < 0.01,
            "{case}: the commit that first shows the ghost must carry its mount-time \
             translate ({GHOST_X}, {GHOST_Y}), got {got:?}"
        );
    }

    /// REGRESSION (the macOS drag ghost painted at the window's top-left for a
    /// frame before jumping under the pointer): AppKit builds a new subview's
    /// layer into the tree inside the commit, and that build reset the layer
    /// transform the mount hook had written. The scene fills a view's handle
    /// before inserting it, so the write lands on a view with no superview.
    fn regression_mount_translate_survives_layer_tree_build(m: &Mounted) {
        assert_placed(
            mount_ghost_and_read_first_commit(m, Placement::AnimatedBeforeInsert),
            "written before insert",
        );
    }

    /// The same, written once the view is in the window but before AppKit
    /// has built its layer in.
    fn regression_translate_after_insert_survives_layer_tree_build(m: &Mounted) {
        assert_placed(
            mount_ghost_and_read_first_commit(m, Placement::AnimatedAfterInsert),
            "written after insert",
        );
    }

    /// A static `transform` on a newly mounted view goes through the same
    /// layer write, so the layer-tree build reset it the same way.
    fn regression_static_transform_survives_layer_tree_build(m: &Mounted) {
        assert_placed(mount_ghost_and_read_first_commit(m, Placement::Static), "static transform");
    }

    pub fn run() {
        // SAFETY: `harness = false` — `main` IS the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        install_commit_probe();
        let m = mount(mtm);
        regression_mount_translate_survives_layer_tree_build(&m);
        regression_translate_after_insert_survives_layer_tree_build(&m);
        regression_static_transform_survives_layer_tree_build(&m);
        println!("mount_translate_first_commit: 3 passed");
    }
}
