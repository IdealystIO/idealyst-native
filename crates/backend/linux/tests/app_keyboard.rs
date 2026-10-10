//! App-level keyboard on GTK: the sink receives key DOWN and UP, with the
//! physical `code`, for keys the window gets — and is torn down on `None`.
//!
//! ## The bug this pins
//!
//! `LinuxBackend` never overrode the app-level key hook, so it inherited
//! the trait's no-op: a game's WASD/arrow listeners heard nothing at all on
//! Linux, and neither the compiler nor any test noticed.
//!
//! GTK4 offers no public way to synthesize a `GdkKeyEvent`, so the test
//! emits the installed controller's own `key-pressed` / `key-released`
//! signals — the exact closures GDK invokes for a real key — and
//! `notify::is-active` for the focus-loss path. That exercises everything
//! from the signal boundary inward (translation, outcome → propagation,
//! teardown); only GDK's event routing to a capture-phase window
//! controller is taken on trust.
//!
//! ONE GTK `#[test]` in this binary: GTK may only be driven from the thread
//! that ran `gtk::init`, and cargo gives every `#[test]` its own thread.

#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::rc::Rc;

use backend_linux::{gtk4, LinuxBackend};
use gtk4::glib::translate::IntoGlib;
use gtk4::prelude::*;
use runtime_shared::primitives::key::{AppKeyEvent, KeyOutcome, KeyPhase, KeyboardSink};
use runtime_vocabulary::caps::AppEnvOps;

#[derive(Debug, PartialEq)]
enum Seen {
    Key(AppKeyEvent),
    FocusLost,
}

fn recording_sink(log: Rc<RefCell<Vec<Seen>>>) -> KeyboardSink {
    let on_lost = log.clone();
    KeyboardSink::new(
        move |ev| {
            log.borrow_mut().push(Seen::Key(ev.clone()));
            // Swallow arrows (a game stopping a scroll); let letters through.
            if ev.code.starts_with("Arrow") {
                KeyOutcome::PreventDefault
            } else {
                KeyOutcome::Default
            }
        },
        move || on_lost.borrow_mut().push(Seen::FocusLost),
    )
}

fn capture_key_controllers(window: &gtk4::Window) -> Vec<gtk4::EventControllerKey> {
    let mut out = Vec::new();
    for c in &window.observe_controllers() {
        if let Ok(k) = c.expect("controller").downcast::<gtk4::EventControllerKey>() {
            if k.propagation_phase() == gtk4::PropagationPhase::Capture {
                out.push(k);
            }
        }
    }
    out
}

fn press(c: &gtk4::EventControllerKey, key: gtk4::gdk::Key, keycode: u32, state: gtk4::gdk::ModifierType) -> bool {
    c.emit_by_name::<bool>("key-pressed", &[&key.into_glib(), &keycode, &state])
}

fn release(c: &gtk4::EventControllerKey, key: gtk4::gdk::Key, keycode: u32, state: gtk4::gdk::ModifierType) {
    c.emit_by_name::<()>("key-released", &[&key.into_glib(), &keycode, &state]);
}

#[test]
fn regression_gtk_app_keyboard_delivers_down_and_up_with_physical_code() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }
    use gtk4::gdk::{Key, ModifierType as M};

    let window = gtk4::Window::new();
    let mut backend = LinuxBackend::new(window.clone());
    let log = Rc::new(RefCell::new(Vec::new()));

    // GtkWindow carries its own capture-phase key controllers (shortcuts,
    // mnemonics), so count relative to that baseline.
    let baseline = capture_key_controllers(&window);
    backend.set_keyboard_sink(Some(recording_sink(log.clone())));
    let ctls: Vec<_> = capture_key_controllers(&window)
        .into_iter()
        .filter(|c| !baseline.contains(c))
        .collect();
    assert_eq!(ctls.len(), 1, "Some installs exactly one capture-phase controller");
    let c = &ctls[0];

    // W down / up: key from the keyval, code from the hardware keycode
    // (evdev KEY_W = 17, + 8).
    assert!(!press(c, Key::w, 17 + 8, M::empty()), "Default → Proceed");
    release(c, Key::w, 17 + 8, M::empty());
    // Arrow: PreventDefault → Stop (the focused entry never sees it).
    assert!(press(c, Key::Up, 103 + 8, M::empty()), "PreventDefault → Stop");
    // Shift itself: GDK reports the pre-event state; the event must carry
    // shift=true on the down, false on the up (Web's convention).
    press(c, Key::Shift_L, 42 + 8, M::empty());
    release(c, Key::Shift_L, 42 + 8, M::SHIFT_MASK);

    let w_down = AppKeyEvent::down("w", "KeyW");
    let w_up = AppKeyEvent::up("w", "KeyW");
    let up_down = AppKeyEvent::down("ArrowUp", "ArrowUp");
    let shift_down = AppKeyEvent { shift: true, ..AppKeyEvent::down("Shift", "ShiftLeft") };
    let shift_up = AppKeyEvent::up("Shift", "ShiftLeft");
    assert_eq!(
        *log.borrow(),
        vec![
            Seen::Key(w_down),
            Seen::Key(w_up),
            Seen::Key(up_down),
            Seen::Key(shift_down),
            Seen::Key(shift_up),
        ]
    );
    assert!(log.borrow().iter().all(|s| !matches!(s, Seen::Key(e) if e.phase == KeyPhase::Up && e.repeat)));

    // Focus loss: an inactive window reports it so held keys are released.
    log.borrow_mut().clear();
    if !window.is_active() {
        window.notify("is-active");
        assert_eq!(*log.borrow(), vec![Seen::FocusLost], "is-active → false reports focus loss");
    } else {
        eprintln!("SKIP(focus section): window is active under this display server");
    }

    // Replacing the sink must not stack a second controller.
    backend.set_keyboard_sink(Some(recording_sink(log.clone())));
    assert_eq!(
        capture_key_controllers(&window).len(),
        baseline.len() + 1,
        "replacement keeps one controller"
    );

    // None tears down both hooks.
    backend.set_keyboard_sink(None);
    assert_eq!(capture_key_controllers(&window), baseline, "None removes the controller");
    log.borrow_mut().clear();
    window.notify("is-active");
    assert!(log.borrow().is_empty(), "None disconnects the is-active watcher");
}
