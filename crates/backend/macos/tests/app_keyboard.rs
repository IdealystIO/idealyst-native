//! The app-level keyboard source against real AppKit (`src/imp/keyboard.rs`):
//! `set_keyboard_sink(Some)` installs an `NSEvent` local monitor that turns
//! key down / key up / flags changed into `AppKeyEvent`s, focus loss reaches
//! `KeyboardSink::focus_lost`, and `set_keyboard_sink(None)` tears all of it
//! down.
//!
//! `harness = false`: AppKit (NSApplication, the main-queue microtask the
//! focus-loss observer defers through) wants the main thread, and libtest
//! runs every case on a worker thread. Events are pushed through
//! `-[NSApplication sendEvent:]`, which is where AppKit runs local monitors.
//!
//! ```sh
//! cargo test -p backend-macos --test app_keyboard
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
    use std::rc::Rc;

    use backend_macos::MacosBackend;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, msg_send_id};
    use objc2_foundation::{CGPoint, MainThreadMarker, NSString};
    use runtime_shared::primitives::key::{AppKeyEvent, KeyOutcome, KeyPhase, KeyboardSink};
    use runtime_vocabulary::caps::AppEnvOps;

    #[allow(non_upper_case_globals)]
    extern "C" {
        static kCFRunLoopDefaultMode: *const std::ffi::c_void;
        fn CFRunLoopRunInMode(mode: *const std::ffi::c_void, seconds: f64, return_after: u8) -> i32;
    }

    const KEY_DOWN: usize = 10;
    const KEY_UP: usize = 11;
    const FLAGS_CHANGED: usize = 12;
    const FLAG_SHIFT: usize = 1 << 17;
    const DEV_LSHIFT: usize = 0x0002;

    fn pump_until(mut done: impl FnMut() -> bool) -> bool {
        const TURNS: usize = 50;
        for _ in 0..TURNS {
            if done() {
                return true;
            }
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, 1) };
        }
        done()
    }

    /// A synthetic key event. `+keyEventWithType:` accepts key-down, key-up
    /// AND flags-changed.
    fn key_event(kind: usize, key_code: u16, flags: usize, chars: &str, repeat: bool) -> Retained<AnyObject> {
        let s = NSString::from_str(chars);
        let ctx: *mut AnyObject = std::ptr::null_mut();
        unsafe {
            msg_send_id![
                class!(NSEvent),
                keyEventWithType: kind,
                location: CGPoint { x: 0.0, y: 0.0 },
                modifierFlags: flags,
                timestamp: 0.0f64,
                windowNumber: 0isize,
                context: ctx,
                characters: &*s,
                charactersIgnoringModifiers: &*s,
                isARepeat: repeat,
                keyCode: key_code,
            ]
        }
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

    struct Recorder {
        events: Rc<RefCell<Vec<AppKeyEvent>>>,
        lost: Rc<Cell<usize>>,
    }

    fn recording_sink() -> (KeyboardSink, Recorder) {
        let events: Rc<RefCell<Vec<AppKeyEvent>>> = Rc::default();
        let lost: Rc<Cell<usize>> = Rc::default();
        let (e, l) = (events.clone(), lost.clone());
        let sink = KeyboardSink::new(
            move |ev| {
                e.borrow_mut().push(ev.clone());
                KeyOutcome::Default
            },
            move || l.set(l.get() + 1),
        );
        (sink, Recorder { events, lost })
    }

    /// REGRESSION (game controls): the app-level hook was key-DOWN only — no
    /// key-up, no modifier keys, no physical code. A real AppKit key-down,
    /// key-up and Shift press/release now all reach the sink.
    fn regression_monitor_delivers_down_up_and_modifiers(app: &AnyObject, backend: &mut MacosBackend) {
        let (sink, rec) = recording_sink();
        backend.set_keyboard_sink(Some(sink));

        send(app, &key_event(KEY_DOWN, 0x0D, 0, "w", false));
        send(app, &key_event(KEY_DOWN, 0x0D, 0, "w", true));
        send(app, &key_event(KEY_UP, 0x0D, 0, "w", false));
        send(app, &key_event(FLAGS_CHANGED, 0x38, FLAG_SHIFT | DEV_LSHIFT, "", false));
        send(app, &key_event(FLAGS_CHANGED, 0x38, 0, "", false));

        let got: Vec<(KeyPhase, String, String, bool)> = rec
            .events
            .borrow()
            .iter()
            .map(|e| (e.phase, e.key.clone(), e.code.clone(), e.repeat))
            .collect();
        let want = vec![
            (KeyPhase::Down, "w".to_string(), "KeyW".to_string(), false),
            (KeyPhase::Down, "w".to_string(), "KeyW".to_string(), true),
            (KeyPhase::Up, "w".to_string(), "KeyW".to_string(), false),
            (KeyPhase::Down, "Shift".to_string(), "ShiftLeft".to_string(), false),
            (KeyPhase::Up, "Shift".to_string(), "ShiftLeft".to_string(), false),
        ];
        assert_eq!(got, want);
        backend.set_keyboard_sink(None);
    }

    /// REGRESSION (game controls): a key held while the app deactivated
    /// stayed "down" forever. App deactivation and a window resigning key
    /// now reach `focus_lost` (the dispatcher synthesizes the releases).
    fn regression_focus_loss_reaches_sink(_app: &AnyObject, backend: &mut MacosBackend) {
        let (sink, rec) = recording_sink();
        backend.set_keyboard_sink(Some(sink));

        post_notification("NSApplicationDidResignActiveNotification");
        assert!(pump_until(|| rec.lost.get() == 1), "app resign-active → focus_lost");
        post_notification("NSWindowDidResignKeyNotification");
        assert!(pump_until(|| rec.lost.get() == 2), "window resign-key → focus_lost");
        backend.set_keyboard_sink(None);
    }

    /// `None` removes the monitor AND the observers: nothing reaches the old
    /// sink afterwards.
    fn none_tears_down_monitor_and_observers(app: &AnyObject, backend: &mut MacosBackend) {
        let (sink, rec) = recording_sink();
        backend.set_keyboard_sink(Some(sink));
        backend.set_keyboard_sink(None);

        send(app, &key_event(KEY_DOWN, 0x00, 0, "a", false));
        post_notification("NSApplicationDidResignActiveNotification");
        pump_until(|| false);
        assert!(rec.events.borrow().is_empty(), "no key after teardown");
        assert_eq!(rec.lost.get(), 0, "no focus_lost after teardown");
    }

    /// A second `Some` swaps the sink in place — one monitor, so each event
    /// is delivered once, to the new sink only.
    fn some_while_installed_swaps_sink(app: &AnyObject, backend: &mut MacosBackend) {
        let (first, rec_first) = recording_sink();
        let (second, rec_second) = recording_sink();
        backend.set_keyboard_sink(Some(first));
        backend.set_keyboard_sink(Some(second));

        send(app, &key_event(KEY_DOWN, 0x31, 0, " ", false));
        assert!(rec_first.events.borrow().is_empty());
        let got = rec_second.events.borrow();
        assert_eq!(got.len(), 1, "delivered exactly once");
        assert_eq!((got[0].key.as_str(), got[0].code.as_str()), (" ", "Space"));
        drop(got);
        backend.set_keyboard_sink(None);
    }

    pub fn run() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        assert!(unsafe { pthread_main_np() } != 0, "app_keyboard must run on the main thread (harness = false)");
        // SAFETY: checked just above — this is the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        backend_macos::install_scheduler();
        let app: Retained<AnyObject> = unsafe { msg_send_id![class!(NSApplication), sharedApplication] };
        let mut backend = MacosBackend::new(mtm);
        let tests: [(&str, fn(&AnyObject, &mut MacosBackend)); 4] = [
            ("regression_monitor_delivers_down_up_and_modifiers", regression_monitor_delivers_down_up_and_modifiers),
            ("regression_focus_loss_reaches_sink", regression_focus_loss_reaches_sink),
            ("none_tears_down_monitor_and_observers", none_tears_down_monitor_and_observers),
            ("some_while_installed_swaps_sink", some_while_installed_swaps_sink),
        ];
        for (name, t) in tests {
            t(&app, &mut backend);
            println!("test {name} ... ok");
        }
        println!("test result: ok. {} passed", tests.len());
    }
}
