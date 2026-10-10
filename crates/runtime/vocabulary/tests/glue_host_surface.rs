//! `glue` must re-export the ambient-host free-function surface.
//!
//! The old `runtime_shared` root carried these for free: its body was
//! `pub use runtime_shared::*;`, so every public item at the shared root
//! (and every public shared module) appeared under `runtime_shared::…`.
//! The facade root (`runtime_vocabulary::glue`) enumerates its exports
//! instead, so a shared item that nobody named simply vanished from the
//! author surface — a silent **product** regression, not a test gap:
//! `runtime_shared::announce(..)`, `runtime_shared::color::parse_or(..)`,
//! `runtime_shared::open_url(..)` and friends stopped resolving for app and
//! SDK crates even though the implementation never moved.
//!
//! These tests are path pins. They assert the item resolves through
//! `glue` AND that it is the same function/type as the shared one, so a
//! future refactor cannot satisfy the path with a divergent shim. Where
//! the behavior is observable without a host (the installer-backed
//! setters), the round-trip is exercised too.

use runtime_vocabulary::glue;

/// `runtime_shared::announce(msg, priority)` — the a11y live-region author
/// surface. Routes to the host-installed announcer; with none installed
/// it is a silent no-op (old-core behavior, unchanged).
#[test]
fn glue_reexports_announce_and_it_routes_to_the_installed_announcer() {
    use std::cell::RefCell;
    use std::rc::Rc;

    // Path pin: the glue item IS the shared item.
    let via_glue: fn(&str, glue::LiveRegionPriority) = glue::announce;
    let via_shared: fn(&str, runtime_shared::accessibility::LiveRegionPriority) =
        runtime_shared::announce;
    assert_eq!(
        via_glue as usize, via_shared as usize,
        "glue::announce must BE runtime_shared::announce, not a reimplementation"
    );

    let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    runtime_shared::host::install_announcer(Some(Rc::new(move |msg: &str, _p| {
        sink.borrow_mut().push(msg.to_string());
    })));
    glue::announce("saved", glue::LiveRegionPriority::Polite);
    runtime_shared::host::install_announcer(None);

    assert_eq!(seen.borrow().as_slice(), ["saved"]);
}

/// `runtime_shared::color_scheme()` and `runtime_shared::host::color_scheme()`
/// — the root re-export and the module path. Both existed on the old
/// root (root export + glob-visible module); only the type was mirrored.
#[test]
fn glue_reexports_color_scheme_at_the_root_and_under_host() {
    runtime_shared::host::install_current_color_scheme(glue::ColorScheme::Dark);
    assert_eq!(glue::color_scheme(), glue::ColorScheme::Dark);
    assert_eq!(glue::host::color_scheme(), glue::ColorScheme::Dark);

    runtime_shared::host::install_current_color_scheme(glue::ColorScheme::Light);
    assert_eq!(glue::color_scheme(), glue::ColorScheme::Light);
    assert_eq!(glue::host::color_scheme(), glue::ColorScheme::Light);
}

/// `runtime_shared::open_url(url)` — routes to the host-installed opener.
#[test]
fn glue_reexports_open_url_and_it_routes_to_the_installed_opener() {
    use std::cell::RefCell;
    use std::rc::Rc;

    let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    runtime_shared::host::install_url_opener(Some(Rc::new(move |u: &str| {
        sink.borrow_mut().push(u.to_string());
    })));
    glue::open_url("https://example.test/a");
    runtime_shared::host::install_url_opener(None);

    assert_eq!(seen.borrow().as_slice(), ["https://example.test/a"]);
    // No opener installed ⇒ silent no-op, not a panic (old-core posture).
    glue::open_url("https://example.test/b");
}

/// `runtime_shared::set_fullscreen(bool)` — routes to the host-installed
/// setter.
#[test]
fn glue_reexports_set_fullscreen_and_it_routes_to_the_installed_setter() {
    use std::cell::Cell;
    use std::rc::Rc;

    let seen: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
    let sink = seen.clone();
    runtime_shared::host::install_fullscreen_setter(Some(Rc::new(move |on: bool| {
        sink.set(Some(on));
    })));
    glue::set_fullscreen(true);
    assert_eq!(seen.get(), Some(true));
    glue::set_fullscreen(false);
    assert_eq!(seen.get(), Some(false));
    runtime_shared::host::install_fullscreen_setter(None);

    // No setter installed ⇒ silent no-op.
    glue::set_fullscreen(true);
}

/// `runtime_shared::color::{parse_or, Rgba}` — the shared color module.
/// This is the one that was actively breaking a build:
/// `crates/sdk/client/canvas/core/src/scene.rs` imports
/// `runtime_shared::color`.
#[test]
fn glue_reexports_the_color_module() {
    let fallback = glue::color::Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    let parsed = glue::color::parse_or("#ff0000", fallback);
    assert_eq!(parsed.r, 255);
    assert_eq!(parsed.g, 0);
    assert_eq!(parsed.b, 0);

    // Unparseable input yields the caller's fallback, unchanged.
    let bad = glue::color::parse_or("not-a-color", fallback);
    assert_eq!((bad.r, bad.g, bad.b, bad.a), (0, 0, 0, 255));

    // Same type by identity — a `Rgba` built through glue is accepted
    // where the shared one is expected.
    let _: runtime_shared::color::Rgba = parsed;
}

/// `glue::set_app_key_handler` IS the shared single-slot installer, so
/// existing author code keeps one behavior regardless of import path.
#[test]
fn glue_reexports_set_app_key_handler() {
    let via_glue: fn(Option<runtime_shared::primitives::key::KeyDownHandler>) =
        glue::set_app_key_handler;
    let via_shared: fn(Option<runtime_shared::primitives::key::KeyDownHandler>) =
        runtime_shared::set_app_key_handler;
    assert_eq!(
        via_glue as usize, via_shared as usize,
        "glue::set_app_key_handler must BE the shared installer"
    );
}

/// Boot a harness the way a real backend's entry does: `install_env_services`
/// wires the keyboard host. Returns the harness with key ops recorded.
fn keyboard_harness() -> host_mock::Harness {
    runtime_shared::key_input::reset_for_tests();
    let h = host_mock::Harness::new();
    h.record_all();
    runtime_vocabulary::backend::install_env_services(&h.backend);
    h
}

fn send_key(h: &host_mock::Harness, ev: runtime_shared::AppKeyEvent) -> runtime_shared::KeyOutcome {
    let sink = h.shared.keyboard_sink.borrow().clone().expect("keyboard sink not installed");
    sink.key(&ev)
}

/// Regression: a handler set from an EVENT CALLBACK (not during a build)
/// never reached the backend. The old single-slot handler was queued and
/// only forwarded by the next style/theme host-state flush; an app whose
/// "Start" button installed key controls and mounted nothing new with a
/// fresh stylesheet got no keys at all, silently. Now installation is
/// immediate — no mount, no flush.
#[test]
fn regression_app_key_handler_installs_without_a_style_flush() {
    use std::rc::Rc;
    let h = keyboard_harness();
    assert!(h.shared.keyboard_sink.borrow().is_none(), "no listener → no key source");

    let handler: runtime_shared::primitives::key::KeyDownHandler =
        Rc::new(|_e| runtime_shared::primitives::key::KeyOutcome::Default);
    glue::set_app_key_handler(Some(handler));
    assert!(
        h.ops().iter().any(|op| op == "set_keyboard_sink some"),
        "installing must reach AppEnvOps::set_keyboard_sink immediately; ops: {:?}",
        h.ops()
    );

    glue::set_app_key_handler(None);
    assert!(
        h.ops().iter().any(|op| op == "set_keyboard_sink none"),
        "clearing the last listener must uninstall the source; ops: {:?}",
        h.ops()
    );
    runtime_shared::key_input::reset_for_tests();
}

/// `on_key` inside a component scope lives exactly as long as the scope:
/// it receives downs AND ups while alive, and dropping the scope removes
/// it (and uninstalls the backend key source).
#[test]
fn on_key_is_scoped_and_sees_up_events() {
    use std::cell::RefCell;
    use std::rc::Rc;
    use runtime_shared::{AppKeyEvent, KeyOutcome, KeyPhase};

    let h = keyboard_harness();
    let seen: Rc<RefCell<Vec<(KeyPhase, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let s = seen.clone();
    h.world.enter(|| {
        let (_, owned) = runtime_world::collect_owned(|| {
            glue::on_key(move |e| {
                s.borrow_mut().push((e.phase, e.code.clone()));
                KeyOutcome::Default
            });
        });
        send_key(&h, AppKeyEvent::down("w", "KeyW"));
        send_key(&h, AppKeyEvent::up("w", "KeyW"));
        assert_eq!(
            *seen.borrow(),
            vec![(KeyPhase::Down, "KeyW".to_string()), (KeyPhase::Up, "KeyW".to_string())]
        );
        drop(owned);
    });
    assert!(
        h.shared.keyboard_sink.borrow().is_none(),
        "dropping the only listener's scope must uninstall the source; ops: {:?}",
        h.ops()
    );
    runtime_shared::key_input::reset_for_tests();
}

/// `key_state()` polls held keys by physical code for the scope's life.
#[test]
fn key_state_polls_held_keys() {
    use runtime_shared::AppKeyEvent;

    let h = keyboard_harness();
    h.world.enter(|| {
        let (keys, owned) = runtime_world::collect_owned(glue::key_state);
        send_key(&h, AppKeyEvent::down("A", "KeyA"));
        assert!(keys.is_down("KeyA"));
        assert!(keys.any_down(&["ArrowLeft", "KeyA"]));
        send_key(&h, AppKeyEvent::up("a", "KeyA"));
        assert!(!keys.is_down("KeyA"));
        drop(owned);
        drop(keys);
    });
    assert!(h.shared.keyboard_sink.borrow().is_none());
    runtime_shared::key_input::reset_for_tests();
}

/// `keyboard_inset()` — the soft-keyboard author surface. Path pin: the
/// glue fn IS the vocabulary's per-world ctx reader and the value type IS
/// the shared `KeyboardInset` (backends push the shared type), so app
/// code spelling `runtime_core::keyboard_inset()` reads what the platform
/// reported.
#[test]
fn glue_reexports_keyboard_inset_and_it_reads_platform_pushes() {
    let via_glue: fn() -> runtime_world::ReadSignal<glue::KeyboardInset> = glue::keyboard_inset;
    let via_vocab: fn() -> runtime_world::ReadSignal<runtime_shared::KeyboardInset> =
        runtime_vocabulary::keyboard::keyboard_inset;
    assert_eq!(via_glue as usize, via_vocab as usize);

    let world = runtime_world::World::new();
    let kb = world.enter(glue::keyboard_inset);
    runtime_vocabulary::keyboard::push(runtime_shared::KeyboardInset::new(
        300.0,
        runtime_shared::keyboard::WEB_KEYBOARD_ESTIMATE,
    ));
    world.flush();
    assert_eq!(world.enter(|| kb.get().height), 300.0);
}
