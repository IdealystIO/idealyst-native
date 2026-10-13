//! The app-wide user-activity source against real AppKit
//! (`src/imp/user_activity.rs`): `install_user_activity_source` adds one
//! `NSEvent` local monitor that ticks `runtime_shared::user_activity` on
//! every press, key press and wheel in ANY of the app's windows, runs the
//! flush hook only when a report ticked, passes every event through
//! untouched, and is removed when the backend drops.
//!
//! `harness = false`: AppKit (NSApplication, windows) wants the main thread,
//! and libtest runs every case on a worker thread. Events are pushed through
//! `-[NSApplication sendEvent:]`, which is where AppKit runs local monitors
//! — the same path a real click or keystroke takes after the run loop
//! dequeues it.
//!
//! Cases run in order and share the process-wide substrate counter, so each
//! asserts on the DELTA of `user_activity::count()`. Every case before
//! `regression_report_is_throttled_and_flush_follows_ticks` runs with no
//! time source installed, so `report()` is never throttled there (see the
//! substrate's module docs); that case installs the clock.
//!
//! ```sh
//! cargo test -p backend-macos --test app_user_activity
//! ```

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    imp::run();
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use backend_macos::MacosBackend;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
    use objc2_app_kit::{NSBackingStoreType, NSTextField, NSView, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSString};
    use runtime_shared::user_activity;

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
        fn CFRelease(cf: *const c_void);
    }

    // NSEventType values.
    const LEFT_DOWN: usize = 1;
    const LEFT_UP: usize = 2;
    const RIGHT_DOWN: usize = 3;
    const RIGHT_UP: usize = 4;
    const MOUSE_MOVED: usize = 5;
    const KEY_DOWN: usize = 10;
    const KEY_UP: usize = 11;
    const FLAGS_CHANGED: usize = 12;
    // CGEventType / button / unit values.
    const CG_OTHER_DOWN: u32 = 25;
    const CG_OTHER_UP: u32 = 26;
    const CG_BUTTON_CENTER: u32 = 2;
    const CG_SCROLL_UNIT_LINE: u32 = 1;
    // Device-independent Shift flag + `kVK_ANSI_A`.
    const FLAG_SHIFT: usize = 1 << 17;
    const KEY_CODE_A: u16 = 0x00;

    static LEFT_DOWNS: AtomicUsize = AtomicUsize::new(0);
    static KEY_DOWNS: AtomicUsize = AtomicUsize::new(0);

    declare_class!(
        /// Counts the presses and key-downs AppKit actually dispatches to it
        /// — the proof that the activity monitor passed the event through.
        struct RecordingView;

        unsafe impl ClassType for RecordingView {
            type Super = NSView;
            type Mutability = mutability::MainThreadOnly;
            const NAME: &'static str = "IdealystUserActivityTestRecordingView";
        }

        impl DeclaredClass for RecordingView {}

        unsafe impl RecordingView {
            #[method(mouseDown:)]
            fn mouse_down(&self, _e: *mut AnyObject) {
                LEFT_DOWNS.fetch_add(1, Ordering::SeqCst);
            }
            #[method(keyDown:)]
            fn key_down(&self, _e: *mut AnyObject) {
                KEY_DOWNS.fetch_add(1, Ordering::SeqCst);
            }
            #[method(acceptsFirstResponder)]
            fn accepts_first_responder(&self) -> bool {
                true
            }
            #[method(acceptsFirstMouse:)]
            fn accepts_first_mouse(&self, _e: *mut AnyObject) -> bool {
                true
            }
        }
    );

    const WIDTH: f64 = 400.0;
    const HEIGHT: f64 = 300.0;

    /// An on-screen window with a full-size [`RecordingView`] as its content
    /// view, offset by `x` so two windows don't overlap.
    fn window(mtm: MainThreadMarker, x: f64) -> (Retained<NSWindow>, Retained<RecordingView>) {
        let rect = CGRect { origin: CGPoint { x, y: 100.0 }, size: CGSize { width: WIDTH, height: HEIGHT } };
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
        let bounds = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: rect.size };
        let rec: Retained<RecordingView> = unsafe { msg_send_id![mtm.alloc::<RecordingView>(), initWithFrame: bounds] };
        window.setContentView(Some(&rec));
        // On screen: AppKit only dispatches a window's mouse events to its
        // views (the pass-through proof) while it is visible.
        let _: () = unsafe { msg_send![&window, orderFrontRegardless] };
        (window, rec)
    }

    fn window_number(w: &NSWindow) -> isize {
        unsafe { msg_send![w, windowNumber] }
    }

    /// A mouse event at the center of `window` (window-base coordinates).
    fn mouse_event(kind: usize, window: &NSWindow) -> Retained<AnyObject> {
        let ctx: *mut AnyObject = std::ptr::null_mut();
        unsafe {
            msg_send_id![
                class!(NSEvent),
                mouseEventWithType: kind,
                location: CGPoint { x: WIDTH / 2.0, y: HEIGHT / 2.0 },
                modifierFlags: 0usize,
                timestamp: 0.0f64,
                windowNumber: window_number(window),
                context: ctx,
                eventNumber: 0isize,
                clickCount: 1isize,
                pressure: 1.0f32,
            ]
        }
    }

    /// A key / flags-changed event addressed to `window`.
    fn key_event(kind: usize, window: &NSWindow, flags: usize, chars: &str) -> Retained<AnyObject> {
        let s = NSString::from_str(chars);
        let ctx: *mut AnyObject = std::ptr::null_mut();
        unsafe {
            msg_send_id![
                class!(NSEvent),
                keyEventWithType: kind,
                location: CGPoint { x: 0.0, y: 0.0 },
                modifierFlags: flags,
                timestamp: 0.0f64,
                windowNumber: window_number(window),
                context: ctx,
                characters: &*s,
                charactersIgnoringModifiers: &*s,
                isARepeat: false,
                keyCode: KEY_CODE_A,
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

    fn middle(kind: u32) -> Retained<AnyObject> {
        let p = CGPoint { x: 0.0, y: 0.0 };
        from_cg(unsafe { CGEventCreateMouseEvent(std::ptr::null(), kind, p, CG_BUTTON_CENTER) })
    }

    fn wheel() -> Retained<AnyObject> {
        from_cg(unsafe { CGEventCreateScrollWheelEvent(std::ptr::null(), CG_SCROLL_UNIT_LINE, 1, -1) })
    }

    fn send(app: &AnyObject, event: &AnyObject) {
        let _: () = unsafe { msg_send![app, sendEvent: event] };
    }

    /// Ticks of the substrate counter while `f` runs.
    fn ticks_during(f: impl FnOnce()) -> u64 {
        let before = user_activity::count();
        f();
        user_activity::count() - before
    }

    struct Fixture {
        mtm: MainThreadMarker,
        app: Retained<AnyObject>,
        host: Retained<NSWindow>,
        /// Flush-hook calls (`after_report`).
        flushes: Rc<Cell<usize>>,
    }

    fn install(backend: &mut MacosBackend, flushes: &Rc<Cell<usize>>) {
        let f = flushes.clone();
        backend.install_user_activity_source(Rc::new(move || f.set(f.get() + 1)));
    }

    /// REGRESSION (kiosk idle clock reset only by bare-background taps):
    /// there was no app-wide activity source — a click on a button or
    /// typing into a field never reached an ancestor's handlers. Every
    /// press-like event (left / right / middle down, key down, wheel) now
    /// ticks once and runs the flush hook; releases, motion and modifier
    /// changes do not tick.
    fn regression_presses_keys_and_wheel_tick(fx: &Fixture, backend: &mut MacosBackend) {
        install(backend, &fx.flushes);
        let flushes_before = fx.flushes.get();
        let app = &*fx.app;
        let w = &*fx.host;
        for (what, event) in [
            ("left down", mouse_event(LEFT_DOWN, w)),
            ("right down", mouse_event(RIGHT_DOWN, w)),
            ("middle down", middle(CG_OTHER_DOWN)),
            ("key down", key_event(KEY_DOWN, w, 0, "a")),
            ("wheel", wheel()),
        ] {
            assert_eq!(ticks_during(|| send(app, &event)), 1, "{what} ticks once");
        }
        assert_eq!(fx.flushes.get() - flushes_before, 5, "every tick runs the flush hook");
        for (what, event) in [
            ("left up", mouse_event(LEFT_UP, w)),
            ("right up", mouse_event(RIGHT_UP, w)),
            ("middle up", middle(CG_OTHER_UP)),
            ("mouse moved", mouse_event(MOUSE_MOVED, w)),
            ("key up", key_event(KEY_UP, w, 0, "a")),
            ("flags changed", key_event(FLAGS_CHANGED, w, FLAG_SHIFT, "")),
        ] {
            assert_eq!(ticks_during(|| send(app, &event)), 0, "{what} does not tick");
        }
    }

    /// REGRESSION (observe only): the monitor must never swallow an event.
    /// A press it counted still reaches the view under it, and a key it
    /// counted still reaches the window's first responder — the same
    /// `sendEvent:` → window → first-responder path a focused
    /// `NSTextField`'s field editor receives typing through.
    fn regression_events_pass_through_unchanged(fx: &Fixture, _backend: &mut MacosBackend) {
        let rec = fx.host.contentView().expect("content view");
        LEFT_DOWNS.store(0, Ordering::SeqCst);
        let ticks = ticks_during(|| send(&fx.app, &mouse_event(LEFT_DOWN, &fx.host)));
        assert_eq!(ticks, 1);
        assert_eq!(LEFT_DOWNS.load(Ordering::SeqCst), 1, "the counted press still reached the view");

        // Key routing goes to the KEY window's first responder. A `cargo
        // test` binary usually stays a background app, so make the host
        // window key explicitly rather than relying on activation.
        let _: () = unsafe { msg_send![&fx.host, makeKeyWindow] };
        let ok: bool = unsafe { msg_send![&fx.host, makeFirstResponder: &*rec] };
        assert!(ok, "the recording view accepts first responder");
        KEY_DOWNS.store(0, Ordering::SeqCst);
        let ticks = ticks_during(|| send(&fx.app, &key_event(KEY_DOWN, &fx.host, 0, "a")));
        assert_eq!(ticks, 1);
        assert_eq!(KEY_DOWNS.load(Ordering::SeqCst), 1, "the counted key still reached the first responder");
    }

    /// REGRESSION (typing resets the idle clock): typing into a focused
    /// `NSTextField` goes `sendEvent:` → key window → the field editor
    /// (`NSTextView`), so the monitor sees each keystroke — it ticks, and the
    /// character still lands in the field.
    fn regression_typing_in_text_field_ticks(fx: &Fixture, _backend: &mut MacosBackend) {
        let frame = CGRect { origin: CGPoint { x: 10.0, y: 10.0 }, size: CGSize { width: 200.0, height: 24.0 } };
        let field: Retained<NSTextField> = unsafe { msg_send_id![fx.mtm.alloc::<NSTextField>(), initWithFrame: frame] };
        let content = fx.host.contentView().expect("content view");
        unsafe { content.addSubview(&field) };
        let _: () = unsafe { msg_send![&fx.host, makeKeyWindow] };
        let ok: bool = unsafe { msg_send![&fx.host, makeFirstResponder: &*field] };
        assert!(ok, "the text field begins editing");
        let ticks = ticks_during(|| send(&fx.app, &key_event(KEY_DOWN, &fx.host, 0, "a")));
        assert_eq!(ticks, 1, "a keystroke into a text field ticks");
        let typed: Retained<NSString> = unsafe { msg_send_id![&field, stringValue] };
        assert_eq!(typed.to_string(), "a", "the keystroke still reached the field");
        let _: bool = unsafe { msg_send![&fx.host, makeFirstResponder: &*content] };
        unsafe { field.removeFromSuperview() };
    }

    /// REGRESSION (portals): a modal or sheet mounts in its own window,
    /// outside the app tree. A press in a SECOND window ticks too, because
    /// the monitor is app-level, not attached to the host window.
    fn regression_second_window_ticks(fx: &Fixture, _backend: &mut MacosBackend) {
        let (other, _rec) = window(fx.mtm, 600.0);
        let ticks = ticks_during(|| send(&fx.app, &mouse_event(LEFT_DOWN, &other)));
        assert_eq!(ticks, 1, "a press in another window of the app ticks");
        other.close();
    }

    /// A second install replaces the first monitor instead of stacking a
    /// second one: one event, one tick.
    fn reinstall_replaces_monitor(fx: &Fixture, backend: &mut MacosBackend) {
        install(backend, &fx.flushes);
        install(backend, &fx.flushes);
        let ticks = ticks_during(|| send(&fx.app, &mouse_event(RIGHT_DOWN, &fx.host)));
        assert_eq!(ticks, 1, "two installs must not double-report");
    }

    /// The flush hook follows `report()`'s answer: with a clock installed, a
    /// burst inside one frame ticks once and schedules one flush.
    fn regression_report_is_throttled_and_flush_follows_ticks(fx: &Fixture, _backend: &mut MacosBackend) {
        runtime_shared::time::install_default_time_source(runtime_shared::Platform::MacOs);
        // The default clock counts from its install, and `0` means "no
        // clock" to the throttle (never throttled, never recorded). Let it
        // leave zero first so the burst below is measured.
        while runtime_shared::time::now_micros() == 0 {
            std::thread::sleep(std::time::Duration::from_micros(1));
        }
        let flushes_before = fx.flushes.get();
        // Back-to-back `sendEvent:` calls land far inside one 60 Hz frame
        // (`MIN_TICK_INTERVAL_MICROS`).
        let ticks = ticks_during(|| {
            send(&fx.app, &wheel());
            send(&fx.app, &wheel());
            send(&fx.app, &wheel());
        });
        assert_eq!(ticks, 1, "a wheel burst inside one frame ticks once");
        assert_eq!(fx.flushes.get() - flushes_before, 1, "a throttled report schedules no flush");
    }

    /// Dropping the backend removes the monitor: later events don't tick.
    fn drop_removes_monitor(fx: &Fixture, backend: MacosBackend) {
        // Step past the throttle window left by the previous case so a
        // still-installed monitor WOULD tick.
        let wait = std::time::Duration::from_micros(2 * user_activity::MIN_TICK_INTERVAL_MICROS);
        std::thread::sleep(wait);
        assert_eq!(ticks_during(|| send(&fx.app, &wheel())), 1, "positive control: installed monitor ticks");
        std::thread::sleep(wait);
        drop(backend);
        assert_eq!(ticks_during(|| send(&fx.app, &wheel())), 0, "a dropped backend's monitor is gone");
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(unsafe { pthread_main_np() } != 0, "app_user_activity must run on the main thread (harness = false)");
        // SAFETY: checked just above — this is the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        let app: Retained<AnyObject> = unsafe { msg_send_id![class!(NSApplication), sharedApplication] };
        let (host, _rec) = window(mtm, 100.0);
        let fx = Fixture { mtm, app, host, flushes: Rc::default() };
        let mut backend = MacosBackend::new(mtm);
        assert_eq!(
            ticks_during(|| send(&fx.app, &mouse_event(LEFT_DOWN, &fx.host))),
            0,
            "nothing reports before the source is installed"
        );
        let tests: [(&str, fn(&Fixture, &mut MacosBackend)); 6] = [
            ("regression_presses_keys_and_wheel_tick", regression_presses_keys_and_wheel_tick),
            ("regression_events_pass_through_unchanged", regression_events_pass_through_unchanged),
            ("regression_typing_in_text_field_ticks", regression_typing_in_text_field_ticks),
            ("regression_second_window_ticks", regression_second_window_ticks),
            ("reinstall_replaces_monitor", reinstall_replaces_monitor),
            (
                "regression_report_is_throttled_and_flush_follows_ticks",
                regression_report_is_throttled_and_flush_follows_ticks,
            ),
        ];
        for (name, t) in tests {
            t(&fx, &mut backend);
            println!("test {name} ... ok");
        }
        drop_removes_monitor(&fx, backend);
        println!("test drop_removes_monitor ... ok");
        fx.host.close();
        println!("test result: ok. {} passed", tests.len() + 1);
    }
}
