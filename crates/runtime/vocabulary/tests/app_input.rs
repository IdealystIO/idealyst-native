//! App situation signals + app-wide pointer input through the author API,
//! against host-mock booted the way a backend boots (`install_env_services`).

use std::cell::RefCell;
use std::rc::Rc;

use runtime_shared::app_state::{self, AppStateChange, PointerKind};
use runtime_shared::pointer_input::{AppPointerEvent, MouseButton, PointerOutcome, PointerPhase};
use runtime_vocabulary::glue;

fn harness() -> host_mock::Harness {
    runtime_shared::pointer_input::reset_for_tests();
    app_state::reset_for_tests();
    let h = host_mock::Harness::new();
    h.record_all();
    runtime_vocabulary::backend::install_env_services(&h.backend);
    h
}

fn sink(h: &host_mock::Harness) -> runtime_shared::pointer_input::PointerSink {
    h.shared.pointer_sink.borrow().clone().expect("pointer sink not installed")
}

fn moved(dx: f32, dy: f32) -> AppPointerEvent {
    AppPointerEvent { dx, dy, ..AppPointerEvent::new(PointerPhase::Move, 0.0, 0.0) }
}

/// A backend reports from a raw platform callback (outside `enter`); the
/// signal follows after the world flushes. This is what lets a game
/// auto-pause on blur (the shim watched `window` blur for it).
#[test]
fn app_state_signals_follow_backend_reports() {
    let h = harness();
    let (focused, visible, rm, ptr, activated) = h.world.enter(|| {
        (glue::app_focused(), glue::app_visible(), glue::reduce_motion(), glue::primary_pointer(), glue::user_activated())
    });
    assert!(h.world.enter(|| focused.get()));
    app_state::report(AppStateChange::Focused(false));
    app_state::report(AppStateChange::Visible(false));
    app_state::report(AppStateChange::ReduceMotion(true));
    app_state::report(AppStateChange::PrimaryPointer(PointerKind::Coarse));
    app_state::report(AppStateChange::UserActivated(false));
    h.world.flush();
    h.world.enter(|| {
        assert!(!focused.get());
        assert!(!visible.get());
        assert!(rm.get());
        assert_eq!(ptr.get(), PointerKind::Coarse);
        assert!(!activated.get());
    });
}

/// CrewForge: an app in Auto kept its launch theme when the OS switched,
/// because `color_scheme()` was read once at mount. A backend's report
/// now reaches the per-world signal (and an effect reading it) on the
/// world's next flush.
#[test]
fn regression_system_color_scheme_follows_a_reported_change() {
    let h = harness();
    runtime_shared::host::report_color_scheme(glue::ColorScheme::Light);
    let seen: Rc<RefCell<Vec<glue::ColorScheme>>> = Rc::default();
    let (scheme, _watch) = h.world.enter(|| {
        let scheme = glue::system_color_scheme();
        let sink = seen.clone();
        (scheme, glue::watch(move || sink.borrow_mut().push(scheme.get())))
    });
    assert_eq!(h.world.enter(|| scheme.get()), glue::ColorScheme::Light);

    runtime_shared::host::report_color_scheme(glue::ColorScheme::Dark);
    h.world.flush();
    assert_eq!(h.world.enter(|| scheme.get()), glue::ColorScheme::Dark);
    assert_eq!(glue::color_scheme(), glue::ColorScheme::Dark, "the plain read agrees");
    assert_eq!(*seen.borrow(), vec![glue::ColorScheme::Light, glue::ColorScheme::Dark], "the effect re-ran");
}

/// The full lock round trip: request reaches the backend synchronously,
/// the backend grants it, `pointer_locked()` flips, the platform drops it
/// (Esc) and the signal flips back.
#[test]
fn pointer_lock_round_trip() {
    let h = harness();
    let locked = h.world.enter(glue::pointer_locked);
    glue::request_pointer_lock();
    assert!(h.ops().iter().any(|op| op == "set_pointer_lock true"), "ops: {:?}", h.ops());
    sink(&h).lock_changed(true);
    h.world.flush();
    assert!(h.world.enter(|| locked.get()));
    sink(&h).lock_changed(false);
    h.world.flush();
    assert!(!h.world.enter(|| locked.get()));
    assert!(h.shared.pointer_sink.borrow().is_none(), "nothing left needing a source");
}

/// Each `pointer_state()` handle accumulates its own movement, so two
/// consumers never steal each other's deltas.
#[test]
fn pointer_state_handles_accumulate_independently() {
    let h = harness();
    h.world.enter(|| {
        let ((a, b), owned) = runtime_world::collect_owned(|| (glue::pointer_state(), glue::pointer_state()));
        sink(&h).event(&moved(3.0, -2.0));
        sink(&h).event(&moved(1.0, 1.0));
        assert_eq!(a.take_movement(), (4.0, -1.0));
        assert_eq!(a.take_movement(), (0.0, 0.0));
        assert_eq!(b.take_movement(), (4.0, -1.0), "b unaffected by a's take");
        sink(&h).event(&AppPointerEvent { wheel_dy: 120.0, ..AppPointerEvent::new(PointerPhase::Wheel, 0.0, 0.0) });
        assert_eq!(a.take_wheel(), (0.0, 120.0));
        sink(&h).event(&AppPointerEvent { button: Some(MouseButton::Primary), ..AppPointerEvent::new(PointerPhase::Down, 0.0, 0.0) });
        assert!(a.is_down(MouseButton::Primary));
        drop(owned);
        drop((a, b));
    });
    assert!(h.shared.pointer_sink.borrow().is_none());
}

/// `on_pointer` is scoped: it fires while its scope lives and is removed
/// (source uninstalled) when the scope drops. Returning PreventDefault on a
/// secondary press is how a game suppresses the context menu.
#[test]
fn on_pointer_is_scoped_and_can_prevent_default() {
    let h = harness();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let s = seen.clone();
    h.world.enter(|| {
        let (_, owned) = runtime_world::collect_owned(|| {
            glue::on_pointer(move |e| {
                s.borrow_mut().push(e.phase);
                if e.button == Some(MouseButton::Secondary) {
                    PointerOutcome::PreventDefault
                } else {
                    PointerOutcome::Default
                }
            });
        });
        let out = sink(&h).event(&AppPointerEvent {
            button: Some(MouseButton::Secondary),
            ..AppPointerEvent::new(PointerPhase::Down, 0.0, 0.0)
        });
        assert_eq!(out, PointerOutcome::PreventDefault);
        drop(owned);
    });
    assert_eq!(*seen.borrow(), vec![PointerPhase::Down]);
    assert!(h.shared.pointer_sink.borrow().is_none());
}

#[test]
fn cursor_hidden_reaches_backend() {
    let h = harness();
    glue::set_cursor_hidden(true);
    assert!(h.ops().iter().any(|op| op == "set_cursor_hidden true"));
    glue::set_cursor_hidden(false);
    assert!(h.ops().iter().any(|op| op == "set_cursor_hidden false"));
}

/// LumiTask's loading screen continues the launch screen; it timed its hold
/// from `app_focused()`, which is the wrong moment. `splash_dismissed()` is
/// `true` from the start where there is no platform splash (host-mock, like
/// web and desktop) and needs no report.
#[test]
fn splash_dismissed_is_true_without_a_platform_splash() {
    let h = harness();
    let splash = h.world.enter(glue::splash_dismissed);
    assert!(h.world.enter(|| splash.get()));
}

/// A phone backend reports `false` at boot and `true` from the first-frame
/// callback (outside `enter`); the signal follows on the world's flush and
/// never goes back to `false`.
#[test]
fn splash_dismissed_follows_the_backend_and_never_reverts() {
    let h = harness();
    app_state::report_splash_dismissed(false); // boot, before the build
    let seen: Rc<RefCell<Vec<bool>>> = Rc::default();
    let (splash, _watch) = h.world.enter(|| {
        let splash = glue::splash_dismissed();
        let sink = seen.clone();
        (splash, glue::watch(move || sink.borrow_mut().push(splash.get())))
    });
    assert!(!h.world.enter(|| splash.get()), "seeded from the boot report");

    app_state::report_splash_dismissed(true); // first frame
    h.world.flush();
    assert!(h.world.enter(|| splash.get()));

    app_state::report_splash_dismissed(false);
    h.world.flush();
    assert!(h.world.enter(|| splash.get()), "never returns to false");
    assert_eq!(*seen.borrow(), vec![false, true]);
}
