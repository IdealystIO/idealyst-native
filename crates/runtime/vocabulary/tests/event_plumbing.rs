//! Event-plumbing installs — the **new-core successor** of the old
//! walker's `key_events.rs` (6), `file_drop.rs` (2) and
//! `scroll_view_on_scroll.rs` (2).
//!
//! What these pin that nothing else does: `caps_conformance.rs` proves
//! each capability is *callable*, and the scene-parity goldens pin the op
//! SEQUENCE — neither shows that the author's closure is the one the
//! backend received, that its RETURN value reaches the platform, or that
//! an absent author slot installs nothing. The platform mechanisms on the
//! far side (web `keydown` + `preventDefault`, UIKit
//! `shouldChangeCharactersIn`, the DOM `dragover`/`drop` pair, macOS
//! `NSDraggingDestination`, native scroll observers) are not reachable
//! from a host test, but they ALL consult exactly the handler these tests
//! fire — which is why the old suites were written this way, and why
//! their absence would be silent.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use host_mock::Harness;
// Paths are spelled `runtime_shared::…` (the permanent substrate the
// vocabulary depends on directly), not through any re-export.
use runtime_shared::primitives::key::{KeyEvent, KeyOutcome};
use runtime_shared::touch::{TouchPoint, TouchResponse};
use runtime_shared::{DroppedFile, FileDropEvent, FileDropPhase};
use runtime_scene::realize;
use runtime_vocabulary::builders::{scroll_view, text, text_area, text_input, view};

fn harness() -> Harness {
    let h = Harness::new();
    h.record_all();
    h
}

fn key(name: &str) -> KeyEvent {
    KeyEvent {
        key: name.to_string(),
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
        selection_start: 0,
        selection_end: 0,
    }
}

// ===========================================================================
// on_key_down — registration, delivery, and the PreventDefault return
// ===========================================================================

#[test]
fn text_input_on_key_down_registers_and_fires() {
    let h = harness();
    let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let recorder = seen.clone();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            text_input()
                .value("")
                .on_key_down(move |e: &KeyEvent| {
                    recorder.borrow_mut().push(e.key.clone());
                    KeyOutcome::Default
                })
                .build(),
        )
    });

    let handler = h
        .key_down_handler(0)
        .expect("create_text_input received the author's on_key_down");
    assert_eq!(handler(&key("Enter")), KeyOutcome::Default);
    assert_eq!(
        seen.borrow().as_slice(),
        ["Enter"],
        "the author closure — not a wrapper that swallows the event — is what fires"
    );
    drop(realized);
}

#[test]
fn on_key_down_prevent_default_propagates_to_the_platform() {
    // The whole point of the return value: a handler that mutates the
    // input imperatively must be able to suppress the platform default
    // (Tab-inserts-spaces, Enter-submits). If the framework dropped the
    // outcome the platform would ALSO act — a silent double-apply.
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            text_area()
                .value("")
                .on_key_down(|e: &KeyEvent| {
                    if e.key == "Tab" {
                        KeyOutcome::PreventDefault
                    } else {
                        KeyOutcome::Default
                    }
                })
                .build(),
        )
    });
    let handler = h.key_down_handler(0).expect("text area on_key_down threaded");
    assert_eq!(handler(&key("Tab")), KeyOutcome::PreventDefault);
    assert_eq!(handler(&key("a")), KeyOutcome::Default);
    drop(realized);
}

#[test]
fn no_on_key_down_means_no_handler_registered() {
    let h = harness();
    let realized = h
        .world
        .enter(|| realize(&h.backend, &h.registry, text_input().value("").build()));
    assert!(
        h.key_down_handler(0).is_none(),
        "an input without on_key_down must register NO handler — a stub handler \
         would have to invent a KeyOutcome and could suppress the platform default"
    );
    drop(realized);
}

#[test]
fn text_area_defaults_to_wrap_and_code_mode_disables_it() {
    // `wrap` / row bounds are create-time config with no update op, so
    // the create call is the only place they are observable — and the
    // default (`wrap = true`) is the contract a code editor opts out of.
    let h = harness();
    let realized = h
        .world
        .enter(|| realize(&h.backend, &h.registry, text_area().value("").build()));
    assert_eq!(
        h.kind_of(0).as_deref(),
        Some("text_area wrap=true min_rows=None max_rows=None"),
        "a plain text area wraps by default"
    );
    drop(realized);

    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            text_area().value("").wrap(false).min_rows(3).build(),
        )
    });
    assert_eq!(
        h.kind_of(0).as_deref(),
        Some("text_area wrap=false min_rows=Some(3) max_rows=None"),
        "code mode threads wrap=false and the row bound to the backend"
    );
    drop(realized);
}

// ===========================================================================
// on_file_drop — install + fire, and the accept-the-drag return value
// ===========================================================================

#[test]
fn plain_view_installs_no_file_drop_handler() {
    let h = harness();
    let realized = h
        .world
        .enter(|| realize(&h.backend, &h.registry, view().build()));
    let log = h.take_log();
    assert!(
        !log.iter().any(|l| l.starts_with("install_file_drop_handler")),
        "a view with no on_file_drop must not subscribe (an unconditional install \
         would make every view accept OS drags): {log:?}"
    );
    drop(realized);
}

#[test]
fn view_on_file_drop_installs_and_fires() {
    let h = harness();
    let dropped: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let recorder = dropped.clone();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            view()
                .on_file_drop(Rc::new(move |e: &FileDropEvent| match &e.phase {
                    // Accepting the drag is a RETURN value, so the
                    // handler's result has to reach the platform (web
                    // `preventDefault`, macOS drag operation).
                    FileDropPhase::Entered => TouchResponse::CONSUMED,
                    FileDropPhase::Dropped(files) => {
                        for f in files {
                            recorder.borrow_mut().push(f.name.clone());
                        }
                        TouchResponse::CONSUMED
                    }
                    _ => TouchResponse::default(),
                }))
                .build(),
        )
    });

    let (node, handler) = h
        .file_drop_handler(0)
        .expect("install_file_drop_handler received the author's handler");
    assert_eq!(node, 0, "installed on the view itself");

    let entered = FileDropEvent {
        phase: FileDropPhase::Entered,
        position: TouchPoint { x: 1.0, y: 2.0 },
    };
    assert!(
        handler(&entered).consumed,
        "the accept-the-drag return value must survive the framework hop"
    );

    let drop_event = FileDropEvent {
        phase: FileDropPhase::Dropped(vec![DroppedFile {
            name: "photo.jpg".into(),
            mime: "image/jpeg".into(),
            size: Some(12),
            path: None,
            source: None,
        }]),
        position: TouchPoint { x: 1.0, y: 2.0 },
    };
    handler(&drop_event);
    assert_eq!(
        dropped.borrow().as_slice(),
        ["photo.jpg"],
        "the dropped file list reaches the author callback intact"
    );
    drop(realized);
}

// ===========================================================================
// scroll_view on_scroll — register + fire with offsets
// ===========================================================================

#[test]
fn scroll_view_on_scroll_registers_and_fires_with_offsets() {
    let h = harness();
    let seen: Rc<RefCell<Vec<(f32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
    let recorder = seen.clone();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            scroll_view()
                .on_scroll(move |x, y| recorder.borrow_mut().push((x, y)))
                .child(text().content("body").build())
                .build(),
        )
    });

    let handler = h
        .scroll_handler(0)
        .expect("create_scroll_view received the author's on_scroll");
    handler(3.0, 42.0);
    assert_eq!(
        seen.borrow().as_slice(),
        [(3.0, 42.0)],
        "both offsets arrive, in (x, y) order — scroll-spy and sticky chrome read them"
    );
    drop(realized);
}

#[test]
fn scroll_view_without_on_scroll_records_absence() {
    let h = harness();
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            scroll_view().child(text().content("body").build()).build(),
        )
    });
    assert_eq!(h.scroll_view_count(), 1, "the scroll view was created");
    assert!(
        h.scroll_handler(0).is_none(),
        "no author on_scroll ⇒ no handler installed (backends skip observer setup entirely)"
    );
    drop(realized);
}

/// The author callback must not outlive its subtree: after teardown the
/// captured state is released, so a late platform scroll report cannot
/// resurrect a dead scope. (Handlers are `Rc`s the backend keeps, so the
/// probe is on the AUTHOR's captured state, which is what a stale
/// callback would touch.)
#[test]
fn on_scroll_author_state_is_released_at_teardown() {
    let h = harness();
    let keep = Rc::new(Cell::new(0u32));
    let weak = Rc::downgrade(&keep);
    let realized = h.world.enter(|| {
        realize(
            &h.backend,
            &h.registry,
            scroll_view()
                .on_scroll(move |_, _| keep.set(keep.get() + 1))
                .build(),
        )
    });
    assert!(weak.upgrade().is_some(), "author state alive while mounted");
    drop(realized);
    // The backend mock deliberately keeps its captured `Rc<dyn Fn>` (a
    // real backend releases it with the node), so the closure itself may
    // outlive the subtree here; what must NOT happen is the framework
    // holding a second, tree-owned copy that keeps firing. Prove the
    // count is exactly one by dropping the mock's capture too.
    h.shared.scroll_handlers.borrow_mut().clear();
    assert!(
        weak.upgrade().is_none(),
        "once the backend releases its handler nothing else holds the author state — \
         the framework installed exactly one copy"
    );
}
