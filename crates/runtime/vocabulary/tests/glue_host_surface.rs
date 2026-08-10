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

/// `runtime_shared::set_app_key_handler(..)` — the free function that
/// installs the app-level key-down handler. The caps side
/// (`AppEnvOps::set_app_key_handler`) was always mirrored; the author
/// entry point was not.
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

    // Install-then-clear must not panic and must leave no handler behind.
    glue::set_app_key_handler(None);
    let _ = runtime_shared::take_pending_app_key_handler();
}

/// The handler installed through `glue::set_app_key_handler` must reach
/// the backend (`AppEnvOps::set_app_key_handler`) on the next host-state
/// flush — the sheet-attach / theme-driver drain.
///
/// The regression: the free fn queues into `runtime_shared`'s pending
/// slot, and under the old core `style::flush_pending_host_state`
/// drained it. The new-core port of that flush
/// (`runtime_vocabulary::theme::flush_pending_host_state`) drained
/// tokens, token updates, default font, app background, and scrollbar —
/// everything EXCEPT the key handler. The caps plumbing existed and was
/// correct on every backend; nothing ever invoked it. Net effect: every
/// app-level shortcut (⌘K, Ctrl-Z, …) was silently dead on every
/// backend, with a clean console — the re-export kept call sites
/// compiling while the queue lost its consumer.
#[test]
fn regression_app_key_handler_installed_through_glue_reaches_the_backend() {
    use std::rc::Rc;

    use host_mock::Harness;
    use runtime_shared::{StyleRules, StyleSheet};
    use runtime_vocabulary::builders::view;

    fn styled() -> runtime_scene::Element {
        let sheet = Rc::new(StyleSheet::r#static(StyleRules::default()));
        view().style(sheet).build()
    }

    let h = Harness::new();
    // `set_app_key_handler` records on the verbose tier.
    h.record_all();

    // Install BEFORE anything mounts — the documented "call once near
    // app start" shape, exactly what the live bug silently dropped.
    let handler: runtime_shared::primitives::key::KeyDownHandler =
        Rc::new(|_e| runtime_shared::primitives::key::KeyOutcome::Default);
    glue::set_app_key_handler(Some(handler));

    // First styled mount runs the host-state flush (sheet attach).
    let _r = h.mount(styled());
    assert!(
        h.ops().iter().any(|op| op == "set_app_key_handler some"),
        "installed handler must be forwarded to AppEnvOps::set_app_key_handler \
         by the first host-state flush; ops: {:?}",
        h.ops()
    );

    // Single-slot: a second flush with nothing queued must not re-send.
    h.clear_ops();
    let _r2 = h.mount(styled());
    assert!(
        !h.ops().iter().any(|op| op.starts_with("set_app_key_handler")),
        "no pending handler → no second backend call; ops: {:?}",
        h.ops()
    );

    // Clearing (`None`) routes through the same drain.
    glue::set_app_key_handler(None);
    h.clear_ops();
    let _r3 = h.mount(styled());
    assert!(
        h.ops().iter().any(|op| op == "set_app_key_handler none"),
        "clearing the handler must also reach the backend; ops: {:?}",
        h.ops()
    );
}
