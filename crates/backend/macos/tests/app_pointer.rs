//! The app-level pointer source against real AppKit (`src/imp/pointer.rs`):
//! `set_pointer_sink(Some)` installs an `NSEvent` local monitor that turns
//! mouse down / up / move / wheel into `AppPointerEvent`s relative to the
//! host root, `PreventDefault` swallows a down before the view's
//! `rightMouseDown:` (so no `menuForEvent:` context menu), pointer lock
//! reports `lock_changed`, focus loss drops the lock, and
//! `set_pointer_sink(None)` tears all of it down.
//!
//! `harness = false`: AppKit (NSApplication, windows, the main-queue
//! microtasks the observers defer through) wants the main thread, and
//! libtest runs every case on a worker thread. Events are pushed through
//! `-[NSApplication sendEvent:]`, which is where AppKit runs local monitors.
//! Motion and wheel events are built as `CGEvent`s so their deltas can be
//! set, then wrapped with `+[NSEvent eventWithCGEvent:]`.
//!
//! The lock GRANT path needs an active app (a lock is refused from the
//! background by design, and that refusal is tested first). The test then
//! asks AppKit to activate it; when the OS declines (a `cargo test` binary
//! usually stays in the background) it lifts the active gate through a
//! doc-hidden test seam so the grant / platform-drop paths still run
//! against real AppKit + CoreGraphics. While locked, the machine's cursor
//! is really frozen for a few milliseconds.
//!
//! ```sh
//! cargo test -p backend-macos --test app_pointer
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use backend_macos::MacosBackend;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
    use objc2_app_kit::{NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSString};
    use runtime_shared::pointer_input::{AppPointerEvent, MouseButton, PointerOutcome, PointerPhase, PointerSink};
    use runtime_vocabulary::caps::AppEnvOps;

    #[allow(non_upper_case_globals)]
    extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after: u8) -> i32;
    }

    /// Opaque `CGEventRef` target, encoded as `^{__CGEvent=}` so objc2's
    /// debug message check accepts it for `+eventWithCGEvent:`.
    #[repr(C)]
    struct CGEvent {
        _p: [u8; 0],
    }
    unsafe impl objc2::encode::RefEncode for CGEvent {
        const ENCODING_REF: objc2::encode::Encoding =
            objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("__CGEvent", &[]));
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreateMouseEvent(source: *const c_void, kind: u32, pos: CGPoint, button: u32) -> *mut CGEvent;
        fn CGEventCreateScrollWheelEvent(source: *const c_void, units: u32, wheel_count: u32, wheel1: i32, ...) -> *mut CGEvent;
        fn CGEventSetIntegerValueField(event: *mut CGEvent, field: u32, value: i64);
        fn CFRelease(cf: *const c_void);
    }

    // NSEventType values.
    const LEFT_DOWN: usize = 1;
    const LEFT_UP: usize = 2;
    const RIGHT_DOWN: usize = 3;
    const RIGHT_UP: usize = 4;
    const KEY_DOWN: usize = 10;
    // CGEventType / field / unit values.
    const CG_MOUSE_MOVED: u32 = 5;
    const CG_OTHER_DOWN: u32 = 25;
    const CG_OTHER_UP: u32 = 26;
    const CG_BUTTON_CENTER: u32 = 2;
    const CG_DELTA_X: u32 = 4;
    const CG_DELTA_Y: u32 = 5;
    const CG_SCROLL_UNIT_LINE: u32 = 1;
    const FLAG_CONTROL: usize = 1 << 18;

    static RIGHT_DOWNS: AtomicUsize = AtomicUsize::new(0);
    static LEFT_DOWNS: AtomicUsize = AtomicUsize::new(0);

    declare_class!(
        /// Counts the presses AppKit actually dispatches to it. Overriding
        /// `rightMouseDown:` without calling super also keeps the Default
        /// case from opening a (blocking) context menu.
        struct RecordingView;

        unsafe impl ClassType for RecordingView {
            type Super = NSView;
            type Mutability = mutability::MainThreadOnly;
            const NAME: &'static str = "IdealystAppPointerTestRecordingView";
        }

        impl DeclaredClass for RecordingView {}

        unsafe impl RecordingView {
            #[method(rightMouseDown:)]
            fn right_mouse_down(&self, _e: *mut AnyObject) {
                RIGHT_DOWNS.fetch_add(1, Ordering::SeqCst);
            }
            #[method(mouseDown:)]
            fn mouse_down(&self, _e: *mut AnyObject) {
                LEFT_DOWNS.fetch_add(1, Ordering::SeqCst);
            }
            #[method(acceptsFirstMouse:)]
            fn accepts_first_mouse(&self, _e: *mut AnyObject) -> bool {
                true
            }
        }
    );

    fn pump_for(seconds: f64) {
        let turns = (seconds / 0.01).ceil() as usize;
        for _ in 0..turns {
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 1) };
        }
    }

    fn pump_until(mut done: impl FnMut() -> bool) -> bool {
        const TURNS: usize = 100;
        for _ in 0..TURNS {
            if done() {
                return true;
            }
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 1) };
        }
        done()
    }

    struct Fixture {
        app: Retained<AnyObject>,
        window: Retained<NSWindow>,
        backend: MacosBackend,
    }

    /// A 400×300 window whose content view is the backend's flipped host
    /// root, with a full-size [`RecordingView`] inside it. Centered on the
    /// current mouse location so a lock never has to warp the user's cursor.
    fn fixture(mtm: MainThreadMarker) -> Fixture {
        let app: Retained<AnyObject> = unsafe { msg_send_id![class!(NSApplication), sharedApplication] };
        let mouse: CGPoint = unsafe { msg_send![class!(NSEvent), mouseLocation] };
        let rect = CGRect {
            origin: CGPoint { x: mouse.x - 200.0, y: mouse.y - 150.0 },
            size: CGSize { width: 400.0, height: 300.0 },
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
        let mut backend = MacosBackend::new(mtm);
        let root = backend.create_host_root().expect("host root");
        let bounds = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: 400.0, height: 300.0 } };
        let _: () = unsafe { msg_send![&root, setFrame: bounds] };
        window.setContentView(Some(&root));
        let rec: Retained<RecordingView> = unsafe { msg_send_id![mtm.alloc::<RecordingView>(), initWithFrame: bounds] };
        unsafe { root.addSubview(&rec) };
        backend.set_host_root(root);
        // On screen: AppKit only dispatches a window's mouse events to its
        // views (the positive control in the PreventDefault case) while it
        // is visible.
        let _: () = unsafe { msg_send![&window, orderFrontRegardless] };
        Fixture { app, window, backend }
    }

    fn window_number(w: &NSWindow) -> isize {
        unsafe { msg_send![w, windowNumber] }
    }

    /// A mouse event at window-base coordinates (bottom-left origin).
    fn mouse_event(kind: usize, window: &NSWindow, x: f64, y: f64, flags: usize) -> Retained<AnyObject> {
        let ctx: *mut AnyObject = std::ptr::null_mut();
        unsafe {
            msg_send_id![
                class!(NSEvent),
                mouseEventWithType: kind,
                location: CGPoint { x, y },
                modifierFlags: flags,
                timestamp: 0.0f64,
                windowNumber: window_number(window),
                context: ctx,
                eventNumber: 0isize,
                clickCount: 1isize,
                pressure: 1.0f32,
            ]
        }
    }

    /// Wrap a CGEvent (consumed) as an NSEvent.
    fn from_cg(cg: *mut CGEvent) -> Retained<AnyObject> {
        assert!(!cg.is_null());
        let ev: Retained<AnyObject> = unsafe { msg_send_id![class!(NSEvent), eventWithCGEvent: cg] };
        unsafe { CFRelease(cg as *const c_void) };
        ev
    }

    /// A CG event at the window's content point `(x, y_from_top)` — CG
    /// positions are global, top-left origin.
    fn cg_point(window: &NSWindow, x: f64, y_from_top: f64) -> CGPoint {
        let frame: CGRect = unsafe { msg_send![window, frame] };
        let content: CGRect = unsafe { msg_send![window, contentRectForFrameRect: frame] };
        let screens: *mut AnyObject = unsafe { msg_send![class!(NSScreen), screens] };
        let primary: *mut AnyObject = unsafe { msg_send![screens, firstObject] };
        let pf: CGRect = unsafe { msg_send![primary, frame] };
        let cocoa_y = content.origin.y + content.size.height - y_from_top;
        CGPoint { x: content.origin.x + x, y: pf.size.height - cocoa_y }
    }

    fn send(app: &AnyObject, event: &AnyObject) {
        let _: () = unsafe { msg_send![app, sendEvent: event] };
    }

    fn post_notification(name: &str) {
        let center: *mut AnyObject = unsafe { msg_send![class!(NSNotificationCenter), defaultCenter] };
        let name = NSString::from_str(name);
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = unsafe { msg_send![center, postNotificationName: &*name, object: nil] };
    }

    #[derive(Clone, Default)]
    struct Recorder {
        events: Rc<RefCell<Vec<AppPointerEvent>>>,
        locks: Rc<RefCell<Vec<bool>>>,
        lost: Rc<Cell<usize>>,
        prevent: Rc<Cell<bool>>,
    }

    fn recording_sink() -> (PointerSink, Recorder) {
        let rec = Recorder::default();
        let (e, l, f, p) = (rec.events.clone(), rec.locks.clone(), rec.lost.clone(), rec.prevent.clone());
        let sink = PointerSink::new(
            move |ev| {
                e.borrow_mut().push(ev.clone());
                if p.get() {
                    PointerOutcome::PreventDefault
                } else {
                    PointerOutcome::Default
                }
            },
            move |locked| l.borrow_mut().push(locked),
            move || f.set(f.get() + 1),
        );
        (sink, rec)
    }

    /// REGRESSION (game controls via a JS shim): there was no app-wide mouse
    /// source on macOS. Real AppKit downs / ups (left, right, and a middle
    /// press from a CGEvent), movement deltas and a notched wheel all reach
    /// the sink, positioned relative to the top-left of the host root.
    fn regression_monitor_delivers_buttons_motion_and_wheel(fx: &mut Fixture) {
        let (sink, rec) = recording_sink();
        fx.backend.set_pointer_sink(Some(sink));
        let accepts: bool = unsafe { msg_send![&fx.window, acceptsMouseMovedEvents] };
        assert!(accepts, "the source turns on mouse-moved delivery for the host window");

        // Window-base (10, 290) = 10 px from the content view's top.
        send(&fx.app, &mouse_event(LEFT_DOWN, &fx.window, 10.0, 290.0, 0));
        send(&fx.app, &mouse_event(LEFT_UP, &fx.window, 10.0, 290.0, 0));
        send(&fx.app, &mouse_event(RIGHT_DOWN, &fx.window, 20.0, 280.0, FLAG_CONTROL));
        send(&fx.app, &mouse_event(RIGHT_UP, &fx.window, 20.0, 280.0, 0));
        unsafe {
            let p = cg_point(&fx.window, 50.0, 60.0);
            send(&fx.app, &from_cg(CGEventCreateMouseEvent(std::ptr::null(), CG_OTHER_DOWN, p, CG_BUTTON_CENTER)));
            send(&fx.app, &from_cg(CGEventCreateMouseEvent(std::ptr::null(), CG_OTHER_UP, p, CG_BUTTON_CENTER)));
            let mv = CGEventCreateMouseEvent(std::ptr::null(), CG_MOUSE_MOVED, p, 0);
            CGEventSetIntegerValueField(mv, CG_DELTA_X, 7);
            CGEventSetIntegerValueField(mv, CG_DELTA_Y, -3);
            send(&fx.app, &from_cg(mv));
            // One notch rolled toward the user (negative in CG/AppKit).
            send(&fx.app, &from_cg(CGEventCreateScrollWheelEvent(std::ptr::null(), CG_SCROLL_UNIT_LINE, 1, -1)));
        }

        let got = rec.events.borrow().clone();
        let buttons: Vec<(PointerPhase, Option<MouseButton>, f32, f32)> = got
            .iter()
            .filter(|e| matches!(e.phase, PointerPhase::Down | PointerPhase::Up))
            .map(|e| (e.phase, e.button, e.x.round(), e.y.round()))
            .collect();
        assert_eq!(
            buttons,
            vec![
                (PointerPhase::Down, Some(MouseButton::Primary), 10.0, 10.0),
                (PointerPhase::Up, Some(MouseButton::Primary), 10.0, 10.0),
                (PointerPhase::Down, Some(MouseButton::Secondary), 20.0, 20.0),
                (PointerPhase::Up, Some(MouseButton::Secondary), 20.0, 20.0),
                (PointerPhase::Down, Some(MouseButton::Middle), 50.0, 60.0),
                (PointerPhase::Up, Some(MouseButton::Middle), 50.0, 60.0),
            ]
        );
        assert!(got.iter().any(|e| e.phase == PointerPhase::Down && e.ctrl), "modifier flags are filled");
        let mv = got.iter().find(|e| e.phase == PointerPhase::Move).expect("a move");
        assert_eq!((mv.dx, mv.dy, mv.x.round(), mv.y.round()), (7.0, -3.0, 50.0, 60.0));
        let wheel = got.iter().find(|e| e.phase == PointerPhase::Wheel).expect("a wheel");
        assert!(wheel.wheel_dy > 0.0, "wheel toward the user is positive like web deltaY (got {})", wheel.wheel_dy);
        assert_eq!(wheel.wheel_dx, 0.0);
        fx.backend.set_pointer_sink(None);
        let accepts: bool = unsafe { msg_send![&fx.window, acceptsMouseMovedEvents] };
        assert!(!accepts, "teardown restores acceptsMouseMovedEvents");
    }

    /// REGRESSION (context menu during play): a claimed secondary press is
    /// swallowed by the monitor, so the view's `rightMouseDown:` — where
    /// NSView asks `menuForEvent:` for the context menu — never runs. An
    /// unclaimed press still reaches the view.
    fn regression_prevent_default_skips_right_mouse_down(fx: &mut Fixture) {
        let (sink, rec) = recording_sink();
        fx.backend.set_pointer_sink(Some(sink));
        RIGHT_DOWNS.store(0, Ordering::SeqCst);
        LEFT_DOWNS.store(0, Ordering::SeqCst);

        rec.prevent.set(true);
        send(&fx.app, &mouse_event(RIGHT_DOWN, &fx.window, 30.0, 30.0, 0));
        send(&fx.app, &mouse_event(RIGHT_UP, &fx.window, 30.0, 30.0, 0));
        send(&fx.app, &mouse_event(LEFT_DOWN, &fx.window, 30.0, 30.0, FLAG_CONTROL));
        send(&fx.app, &mouse_event(LEFT_UP, &fx.window, 30.0, 30.0, 0));
        assert_eq!(RIGHT_DOWNS.load(Ordering::SeqCst), 0, "claimed right press never reaches rightMouseDown:");
        assert_eq!(LEFT_DOWNS.load(Ordering::SeqCst), 0, "claimed (ctrl-)press never reaches mouseDown:");

        rec.prevent.set(false);
        send(&fx.app, &mouse_event(RIGHT_DOWN, &fx.window, 30.0, 30.0, 0));
        send(&fx.app, &mouse_event(RIGHT_UP, &fx.window, 30.0, 30.0, 0));
        assert_eq!(RIGHT_DOWNS.load(Ordering::SeqCst), 1, "an unclaimed press is dispatched normally");
        fx.backend.set_pointer_sink(None);
    }

    /// REGRESSION (stuck button): a control's tracking loop eats the
    /// mouse-up before the monitor sees it. A down whose release never
    /// arrives (and the hardware reports released) gets a synthesized Up.
    fn regression_eaten_release_is_synthesized(fx: &mut Fixture) {
        let (sink, rec) = recording_sink();
        fx.backend.set_pointer_sink(Some(sink));
        send(&fx.app, &mouse_event(LEFT_DOWN, &fx.window, 5.0, 5.0, 0));
        assert!(
            pump_until(|| rec.events.borrow().iter().any(|e| e.phase == PointerPhase::Up)),
            "synthesized Up for the eaten release"
        );
        let ups: Vec<_> = rec.events.borrow().iter().filter(|e| e.phase == PointerPhase::Up).map(|e| e.button).collect();
        assert_eq!(ups, vec![Some(MouseButton::Primary)]);
        fx.backend.set_pointer_sink(None);
    }

    /// From the background a lock is refused (`false`), the cursor is not
    /// hidden and nothing is held.
    fn lock_refused_while_inactive(fx: &mut Fixture) {
        let (sink, rec) = recording_sink();
        fx.backend.set_pointer_sink(Some(sink));
        fx.backend.set_pointer_lock(true);
        assert_eq!(*rec.locks.borrow(), vec![false]);
        assert!(!fx.backend.pointer_lock_held());
        assert!(!fx.backend.cursor_hidden_by_pointer_source());
        fx.backend.set_pointer_sink(None);
    }

    /// Lock granted synchronously (cursor hidden), then dropped by the
    /// platform when the app resigns active — reported, cursor restored.
    fn regression_lock_dropped_on_resign_active(fx: &mut Fixture) {
        let (sink, rec) = recording_sink();
        fx.backend.set_pointer_sink(Some(sink));
        fx.backend.set_pointer_lock(true);
        assert_eq!(*rec.locks.borrow(), vec![true], "granted synchronously");
        assert!(fx.backend.pointer_lock_held() && fx.backend.cursor_hidden_by_pointer_source());

        post_notification("NSApplicationDidResignActiveNotification");
        assert!(pump_until(|| rec.locks.borrow().len() == 2), "resign-active drops the lock");
        assert_eq!(*rec.locks.borrow(), vec![true, false]);
        assert_eq!(rec.lost.get(), 1, "and reports focus loss");
        assert!(!fx.backend.pointer_lock_held() && !fx.backend.cursor_hidden_by_pointer_source());
        fx.backend.set_pointer_sink(None);
    }

    /// Esc drops a held lock (the browser's escape hatch, kept identical).
    fn lock_dropped_on_escape(fx: &mut Fixture) {
        let (sink, rec) = recording_sink();
        fx.backend.set_pointer_sink(Some(sink));
        fx.backend.set_pointer_lock(true);
        let s = NSString::from_str("\u{1b}");
        let ctx: *mut AnyObject = std::ptr::null_mut();
        let esc: Retained<AnyObject> = unsafe {
            msg_send_id![
                class!(NSEvent),
                keyEventWithType: KEY_DOWN,
                location: CGPoint { x: 0.0, y: 0.0 },
                modifierFlags: 0usize,
                timestamp: 0.0f64,
                windowNumber: 0isize,
                context: ctx,
                characters: &*s,
                charactersIgnoringModifiers: &*s,
                isARepeat: false,
                keyCode: 0x35u16,
            ]
        };
        send(&fx.app, &esc);
        assert_eq!(*rec.locks.borrow(), vec![true, false]);
        assert!(!fx.backend.pointer_lock_held());
        fx.backend.set_pointer_sink(None);
    }

    /// Teardown with a lock held AND the cursor hidden releases both
    /// (balanced unhide), removes the monitor, and reports nothing more.
    fn teardown_releases_lock_cursor_and_monitor(fx: &mut Fixture) {
        let (sink, rec) = recording_sink();
        fx.backend.set_pointer_sink(Some(sink));
        fx.backend.set_cursor_hidden(true);
        assert!(fx.backend.cursor_hidden_by_pointer_source());
        fx.backend.set_pointer_lock(true);
        assert!(fx.backend.pointer_lock_held());
        fx.backend.set_pointer_sink(None);
        assert!(!fx.backend.pointer_lock_held());
        assert!(!fx.backend.cursor_hidden_by_pointer_source(), "cursor shown again");
        // The dispatcher's trailing `HideCursor(false)` after teardown is a no-op.
        fx.backend.set_cursor_hidden(false);
        assert!(!fx.backend.cursor_hidden_by_pointer_source());

        let before = rec.events.borrow().len();
        send(&fx.app, &mouse_event(LEFT_DOWN, &fx.window, 5.0, 5.0, 0));
        send(&fx.app, &mouse_event(LEFT_UP, &fx.window, 5.0, 5.0, 0));
        post_notification("NSApplicationDidResignActiveNotification");
        pump_for(0.2);
        assert_eq!(rec.events.borrow().len(), before, "no event after teardown");
        assert_eq!(rec.lost.get(), 0, "no focus_lost after teardown");
        assert_eq!(rec.locks.borrow().iter().filter(|l| !**l).count(), 0, "no lock report after teardown");
    }

    /// A second `Some` swaps the sink in place — one monitor, each event
    /// delivered once, to the new sink only.
    fn some_while_installed_swaps_sink(fx: &mut Fixture) {
        let (first, rec_first) = recording_sink();
        let (second, rec_second) = recording_sink();
        fx.backend.set_pointer_sink(Some(first));
        fx.backend.set_pointer_sink(Some(second));
        send(&fx.app, &mouse_event(LEFT_DOWN, &fx.window, 5.0, 5.0, 0));
        send(&fx.app, &mouse_event(LEFT_UP, &fx.window, 5.0, 5.0, 0));
        assert!(rec_first.events.borrow().is_empty());
        assert_eq!(rec_second.events.borrow().len(), 2);
        fx.backend.set_pointer_sink(None);
    }

    /// App situation: the boot reading matches AppKit, and each followed
    /// notification — including the reduce-motion one, which is posted on
    /// NSWorkspace's OWN center — re-reads and runs the flush callback.
    fn app_state_reads_at_boot_and_follows_notifications(fx: &mut Fixture) {
        use runtime_shared::app_state;
        app_state::reset_for_tests();
        let flushes = Rc::new(Cell::new(0usize));
        let f = flushes.clone();
        fx.backend.install_app_state_observers(Rc::new(move || f.set(f.get() + 1)));
        let active: bool = unsafe { msg_send![&fx.app, isActive] };
        let ws: *mut AnyObject = unsafe { msg_send![class!(NSWorkspace), sharedWorkspace] };
        let reduce: bool = unsafe { msg_send![ws, accessibilityDisplayShouldReduceMotion] };
        let st = app_state::current();
        assert_eq!(st.focused, active);
        assert_eq!(st.reduce_motion, reduce);
        assert_eq!(st.primary_pointer, app_state::PointerKind::Fine);

        post_notification("NSApplicationDidBecomeActiveNotification");
        assert!(pump_until(|| flushes.get() == 1), "focus notification → report + flush");
        post_notification("NSApplicationDidChangeOcclusionStateNotification");
        assert!(pump_until(|| flushes.get() == 2), "occlusion notification → report + flush");
        let ws_center: *mut AnyObject = unsafe { msg_send![ws, notificationCenter] };
        let name = NSString::from_str("NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification");
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = unsafe { msg_send![ws_center, postNotificationName: &*name, object: nil] };
        assert!(pump_until(|| flushes.get() == 3), "workspace-center reduce-motion notification → report + flush");
    }

    fn try_activate(fx: &Fixture) -> bool {
        unsafe {
            // NSApplicationActivationPolicyAccessory: may activate without a
            // Dock icon.
            let _: bool = msg_send![&fx.app, setActivationPolicy: 1isize];
            let _: () = msg_send![&fx.window, makeKeyAndOrderFront: std::ptr::null::<AnyObject>()];
            let _: () = msg_send![&fx.app, activateIgnoringOtherApps: true];
        }
        pump_until(|| unsafe { msg_send![&fx.app, isActive] })
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(unsafe { pthread_main_np() } != 0, "app_pointer must run on the main thread (harness = false)");
        // SAFETY: checked just above — this is the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        let mut fx = fixture(mtm);
        let mut passed = 0;
        let mut run = |name: &str, t: &mut dyn FnMut(&mut Fixture), fx: &mut Fixture| {
            t(fx);
            println!("test {name} ... ok");
            passed += 1;
        };
        run("regression_monitor_delivers_buttons_motion_and_wheel", &mut regression_monitor_delivers_buttons_motion_and_wheel, &mut fx);
        run("regression_prevent_default_skips_right_mouse_down", &mut regression_prevent_default_skips_right_mouse_down, &mut fx);
        run("regression_eaten_release_is_synthesized", &mut regression_eaten_release_is_synthesized, &mut fx);
        run("some_while_installed_swaps_sink", &mut some_while_installed_swaps_sink, &mut fx);
        run("app_state_reads_at_boot_and_follows_notifications", &mut app_state_reads_at_boot_and_follows_notifications, &mut fx);
        let active: bool = unsafe { msg_send![&fx.app, isActive] };
        if !active {
            run("lock_refused_while_inactive", &mut lock_refused_while_inactive, &mut fx);
        }
        // Prefer a really active app; otherwise lift the active gate so the
        // grant / drop paths still run against real AppKit + CoreGraphics.
        if !try_activate(&fx) {
            println!("(the OS did not activate the test app; lifting the active gate for the lock cases)");
            fx.backend.pointer_lock_skip_active_gate_for_tests(true);
        }
        run("regression_lock_dropped_on_resign_active", &mut regression_lock_dropped_on_resign_active, &mut fx);
        run("lock_dropped_on_escape", &mut lock_dropped_on_escape, &mut fx);
        run("teardown_releases_lock_cursor_and_monitor", &mut teardown_releases_lock_cursor_and_monitor, &mut fx);
        println!("test result: ok. {passed} passed");
    }
}
