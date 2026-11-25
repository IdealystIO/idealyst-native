//! The macOS animation clock (`raf_loop`, `after_animation_frame`) against
//! real AppKit, a real window on screen and a real run loop.
//!
//! The clock is a `CADisplayLink` vended by the backend's host root view
//! (macOS 14+), with an NSTimer backstop for when the link is silent. These
//! cases check that it ticks at the display's refresh rate (on a 120 Hz
//! ProMotion screen a 60 Hz timer gave a 220 ms tween 12 ticks against ~26
//! refreshes, unevenly spaced), that it stops when idle, that
//! `after_animation_frame` lands on the next frame, and that a window that is
//! not on screen still gets backstop ticks.
//!
//! `harness = false`: AppKit windows and the display link want the main
//! thread, and libtest runs every case on a worker thread. The window is
//! ordered front without activating the app.
//!
//! ```sh
//! cargo test -p backend-macos --test raf_display_rate
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
    use std::time::{Duration, Instant};

    use backend_apple_core::scheduler::{frame_clock_has_display_link, frame_clock_is_running};
    use backend_macos::MacosBackend;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};
    use objc2_app_kit::{NSBackingStoreType, NSWindow, NSWindowStyleMask};
    use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker};

    #[allow(non_upper_case_globals)]
    extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after: u8) -> i32;
    }

    /// How long each rate measurement runs.
    const MEASURE_S: f64 = 0.5;

    fn spin(seconds: f64) {
        let until = Instant::now() + Duration::from_secs_f64(seconds);
        while Instant::now() < until {
            let left = until.saturating_duration_since(Instant::now()).as_secs_f64();
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, left.max(0.001), 0) };
        }
    }

    struct Fixture {
        window: Retained<NSWindow>,
        _backend: MacosBackend,
    }

    fn fixture(mtm: MainThreadMarker) -> Fixture {
        let _app: Retained<AnyObject> =
            unsafe { msg_send_id![class!(NSApplication), sharedApplication] };
        let rect = CGRect {
            origin: CGPoint { x: 200.0, y: 200.0 },
            size: CGSize { width: 320.0, height: 200.0 },
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
        let bounds = CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: rect.size };
        let _: () = unsafe { msg_send![&root, setFrame: bounds] };
        window.setContentView(Some(&root));
        // Registers the host root as the clock's display-link owner.
        backend.set_host_root(root);
        // On a display: the view's link only calls back while it is.
        let _: () = unsafe { msg_send![&window, orderFrontRegardless] };
        spin(0.2);
        Fixture { window, _backend: backend }
    }

    /// The refresh rate of the screen the window is on, or `None` when the OS
    /// can't say (`maximumFramesPerSecond` is macOS 12+).
    fn window_hz(window: &NSWindow) -> Option<f64> {
        let screen: Option<Retained<AnyObject>> = unsafe { msg_send_id![window, screen] };
        let screen = screen?;
        let fps: isize = unsafe { msg_send![&*screen, maximumFramesPerSecond] };
        (fps > 0).then_some(fps as f64)
    }

    /// Run a `raf_loop` for `seconds` and return the tick instants.
    fn measure(seconds: f64) -> Vec<Instant> {
        let ticks: Rc<RefCell<Vec<Instant>>> = Rc::new(RefCell::new(Vec::new()));
        let t = ticks.clone();
        let raf = runtime_shared::scheduling::raf_loop(move || t.borrow_mut().push(Instant::now()));
        // Let the first tick arrive, then measure whole frames from there.
        spin(0.05);
        ticks.borrow_mut().clear();
        spin(seconds);
        drop(raf);
        let out = ticks.borrow().clone();
        out
    }

    fn median_interval_ms(ticks: &[Instant]) -> f64 {
        let mut d: Vec<f64> =
            ticks.windows(2).map(|w| (w[1] - w[0]).as_secs_f64() * 1000.0).collect();
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        d.get(d.len() / 2).copied().unwrap_or(f64::NAN)
    }

    /// REGRESSION (animations judder on 120 Hz ProMotion): the clock was a
    /// 60 Hz NSTimer, so a 120 Hz display got half the ticks it can show, and
    /// they landed unevenly against vsync. The clock must tick once per
    /// display refresh.
    fn regression_raf_loop_ticks_at_the_display_rate(fx: &Fixture) {
        let hz = window_hz(&fx.window).unwrap_or(60.0);
        let ticks = measure(MEASURE_S);
        let got = ticks.len() as f64 / MEASURE_S;
        let median = median_interval_ms(&ticks);
        println!(
            "  display {hz:.0} Hz, display link: {}, {got:.0} ticks/s, median interval {median:.2} ms",
            frame_clock_has_display_link_after_start()
        );
        assert!(
            got >= hz * 0.8,
            "the clock must tick at the display rate ({hz:.0} Hz): got {got:.0} ticks/s"
        );
        assert!(
            (median - 1000.0 / hz).abs() < 1000.0 / hz * 0.3,
            "ticks must be one display frame apart ({:.2} ms): median {median:.2} ms",
            1000.0 / hz
        );
        if hz <= 61.0 {
            println!("  (60 Hz screen: the above-60 Hz check is skipped)");
            return;
        }
        assert!(
            got > 60.0 * 1.3,
            "a {hz:.0} Hz display must get more ticks than a 60 Hz timer gives: {got:.0}/s"
        );
    }

    /// Whether a started clock is display-synced. `false` only below macOS 14,
    /// where the clock runs on its NSTimer at the screen's rate.
    fn frame_clock_has_display_link_after_start() -> bool {
        let raf = runtime_shared::scheduling::raf_loop(|| {});
        let linked = frame_clock_has_display_link();
        drop(raf);
        linked
    }

    /// On macOS 14+ the clock's frames come from the display link, not the
    /// timer.
    fn the_clock_is_display_synced_on_macos_14(_fx: &Fixture) {
        let responds: bool = unsafe {
            msg_send![
                class!(NSView),
                instancesRespondToSelector: objc2::sel!(displayLinkWithTarget:selector:)
            ]
        };
        if !responds {
            println!("  (macOS < 14: no view display link, the timer drives the clock; skipped)");
            return;
        }
        assert!(
            frame_clock_has_display_link_after_start(),
            "on macOS 14+ the clock must use the host root's display link"
        );
    }

    /// No idle CPU: once the last loop is dropped nothing is scheduled and no
    /// tick runs.
    fn the_clock_stops_when_nothing_is_subscribed(_fx: &Fixture) {
        let n = Rc::new(Cell::new(0u32));
        let n2 = n.clone();
        let raf = runtime_shared::scheduling::raf_loop(move || n2.set(n2.get() + 1));
        spin(0.1);
        assert!(n.get() > 0, "the loop ticked");
        assert!(frame_clock_is_running());
        drop(raf);
        assert!(!frame_clock_is_running(), "dropping the last loop stops the source");
        let at_drop = n.get();
        spin(0.1);
        assert_eq!(n.get(), at_drop, "no tick after the loop was dropped");
    }

    /// `after_animation_frame` fires on the next frame, then the clock idles.
    fn after_animation_frame_lands_on_the_next_frame(fx: &Fixture) {
        let hz = window_hz(&fx.window).unwrap_or(60.0);
        let fired: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        let f = fired.clone();
        let asked = Instant::now();
        let task = runtime_shared::scheduling::after_animation_frame(move || f.set(Some(Instant::now())));
        let mut waited = 0;
        while fired.get().is_none() && waited < 100 {
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.002, 1) };
            waited += 1;
        }
        let at = fired.get().expect("after_animation_frame fired");
        let ms = (at - asked).as_secs_f64() * 1000.0;
        println!("  after_animation_frame fired after {ms:.2} ms");
        assert!(ms < 3.0 * 1000.0 / hz, "within a few frames of the request: {ms:.2} ms");
        drop(task);
        assert!(!frame_clock_is_running(), "a fired one-shot leaves the clock idle");
    }

    /// A window that is not on screen: the view's link is silent there, and
    /// the NSTimer backstop must keep the clock ticking (anchored-portal
    /// trackers, presence one-shots and tweens in a hidden window).
    fn a_window_off_screen_still_gets_backstop_ticks(fx: &Fixture) {
        let _: () = unsafe { msg_send![&fx.window, orderOut: std::ptr::null::<AnyObject>()] };
        spin(0.1);
        let ticks = measure(MEASURE_S);
        let got = ticks.len() as f64 / MEASURE_S;
        println!("  off screen: {got:.0} ticks/s");
        assert!(got >= 30.0, "the backstop must tick an off-screen window's clock: {got:.0}/s");
        let _: () = unsafe { msg_send![&fx.window, orderFrontRegardless] };
        spin(0.1);
    }

    /// The clock follows the window to a screen with a different refresh rate.
    /// Needs two screens whose rates differ; skipped otherwise.
    fn the_clock_follows_the_window_to_another_screen(fx: &Fixture) {
        let screens: Retained<AnyObject> = unsafe { msg_send_id![class!(NSScreen), screens] };
        let count: usize = unsafe { msg_send![&*screens, count] };
        let here = window_hz(&fx.window).unwrap_or(60.0);
        let mut other: Option<(Retained<AnyObject>, f64)> = None;
        for i in 0..count {
            let s: Retained<AnyObject> = unsafe { msg_send_id![&*screens, objectAtIndex: i] };
            let fps: isize = unsafe { msg_send![&*s, maximumFramesPerSecond] };
            if fps > 0 && (fps as f64 - here).abs() > 5.0 {
                other = Some((s, fps as f64));
                break;
            }
        }
        let Some((screen, hz)) = other else {
            println!("  (no second screen with a different refresh rate; skipped)");
            return;
        };
        let frame: CGRect = unsafe { msg_send![&*screen, visibleFrame] };
        let origin = CGPoint { x: frame.origin.x + 100.0, y: frame.origin.y + 100.0 };
        let _: () = unsafe { msg_send![&fx.window, setFrameOrigin: origin] };
        spin(0.3);
        let ticks = measure(MEASURE_S);
        let got = ticks.len() as f64 / MEASURE_S;
        println!("  moved to a {hz:.0} Hz screen: {got:.0} ticks/s");
        assert!(
            (got - hz).abs() < hz * 0.2,
            "after moving to a {hz:.0} Hz screen the clock must tick at its rate: {got:.0}/s"
        );
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(
            unsafe { pthread_main_np() } != 0,
            "raf_display_rate must run on the main thread (harness = false)"
        );
        // SAFETY: checked just above.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        let fx = fixture(mtm);
        let cases: [(&str, fn(&Fixture)); 6] = [
            ("the_clock_is_display_synced_on_macos_14", the_clock_is_display_synced_on_macos_14),
            ("regression_raf_loop_ticks_at_the_display_rate", regression_raf_loop_ticks_at_the_display_rate),
            ("the_clock_stops_when_nothing_is_subscribed", the_clock_stops_when_nothing_is_subscribed),
            ("after_animation_frame_lands_on_the_next_frame", after_animation_frame_lands_on_the_next_frame),
            ("a_window_off_screen_still_gets_backstop_ticks", a_window_off_screen_still_gets_backstop_ticks),
            ("the_clock_follows_the_window_to_another_screen", the_clock_follows_the_window_to_another_screen),
        ];
        for (name, case) in cases {
            case(&fx);
            println!("test {name} ... ok");
        }
        fx.window.close();
        println!("\ntest result: ok. {} passed", cases.len());
    }
}
