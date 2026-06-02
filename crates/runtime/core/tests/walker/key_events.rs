//! `on_key_down` plumbing — assert that a handler attached to a
//! `TextInput` or `TextArea` survives mount, gets registered with the
//! backend, and fires with the canonical `KeyEvent` shape when the
//! backend dispatches one.
//!
//! These tests use `MockBackend::fire_key_event` to synthesize a key
//! press without going through a real platform — what we're verifying
//! is the framework wiring (primitive variant → walker → backend
//! `create_text_*` → registered handler), not the per-backend keydown
//! listener (those live in each backend's own integration tests).

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::primitives::key::{KeyEvent, KeyOutcome};
use runtime_core::{signal, text_area, text_input, Signal};

use crate::common::{Event, NodeId, TestRuntime};

fn make_key_event(key: &str) -> KeyEvent {
    KeyEvent {
        key: key.to_string(),
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
        selection_start: 0,
        selection_end: 0,
    }
}

fn first_node_id<F: Fn(&Event) -> bool>(events: &[Event], _pred: F) -> Option<NodeId> {
    // The Mock backend mints IDs starting at 0; a freshly-created
    // text input/area is the first non-View node minted by these
    // tests, so reading the first emitted NodeId off the create
    // event would be more robust — but the tests below only need
    // *some* node id that has a registered handler. The mock backend
    // hands back the minted id from `create_*`, but we don't capture
    // it directly through the public surface; instead we know
    // (because we only mount one input per test) it's NodeId(0).
    Some(NodeId(0))
}

/// `text_input` carrying an `on_key_down` records a CreateTextInput
/// event with `has_key_handler: true`, and the handler can be
/// invoked via `fire_key_event` on the mock backend.
#[test]
fn text_input_on_key_down_registers_and_fires() {
    let rt = TestRuntime::new();
    let fired: Rc<RefCell<Vec<KeyEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let fired_clone = fired.clone();

    let value: Signal<String> = signal!(String::new());
    let on_change = move |_: String| {};
    let on_key_down = move |ev: &KeyEvent| {
        fired_clone.borrow_mut().push(ev.clone());
        KeyOutcome::Default
    };

    let _owner = rt.render(
        text_input(value, on_change)
            .on_key_down(on_key_down)
            .into(),
    );

    // Mount-side: the create event records the handler presence.
    rt.backend().assert_any(|e| {
        matches!(
            e,
            Event::CreateTextInput {
                has_key_handler: true,
                ..
            }
        )
    });

    // Find the node id (only one TextInput mounted; mock minted
    // starting at 0).
    let events = rt.events();
    let node = first_node_id(&events, |e| {
        matches!(e, Event::CreateTextInput { .. })
    })
    .expect("expected a TextInput in the event log");

    // Synthesize a Tab keydown via the mock helper; the handler
    // should observe the exact `KeyEvent` we passed in.
    let ev = make_key_event("Tab");
    let outcome = rt.backend().fire_key_event(node, &ev);
    assert_eq!(outcome, Some(KeyOutcome::Default));
    let received = fired.borrow();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].key, "Tab");
}

/// The `secure` flag threads from the primitive builder through the walker
/// into `create_text_input`. A bare `text_input` defaults to `secure: false`
/// (plaintext); `.secure(true)` records `secure: true`. Without this wiring
/// password fields would render unmasked on every backend.
#[test]
fn text_input_secure_flag_threads_to_backend() {
    // Default: not secure.
    let rt = TestRuntime::new();
    let value: Signal<String> = signal!(String::new());
    let _owner = rt.render(text_input(value, |_: String| {}).into());
    rt.backend()
        .assert_any(|e| matches!(e, Event::CreateTextInput { secure: false, .. }));

    // Opted in via the builder.
    let rt2 = TestRuntime::new();
    let value2: Signal<String> = signal!(String::new());
    let _owner2 = rt2.render(text_input(value2, |_: String| {}).secure(true).into());
    rt2.backend()
        .assert_any(|e| matches!(e, Event::CreateTextInput { secure: true, .. }));
}

/// Same for `text_area`. Verifies returning `PreventDefault`
/// propagates back through `fire_key_event`'s result so a real
/// backend would call `event.preventDefault()`.
#[test]
fn text_area_on_key_down_prevent_default_propagates() {
    let rt = TestRuntime::new();
    let value: Signal<String> = signal!(String::new());
    let on_change = move |_: String| {};
    let on_key_down = |_: &KeyEvent| KeyOutcome::PreventDefault;

    let _owner = rt.render(
        text_area(value, on_change)
            .on_key_down(on_key_down)
            .into(),
    );

    rt.backend().assert_any(|e| {
        matches!(
            e,
            Event::CreateTextArea {
                has_key_handler: true,
                ..
            }
        )
    });

    let outcome = rt
        .backend()
        .fire_key_event(NodeId(0), &make_key_event("Tab"));
    assert_eq!(outcome, Some(KeyOutcome::PreventDefault));
}

/// The `wrap` flag threads from the primitive builder through the
/// walker into `create_text_area`. A bare `text_area` defaults to
/// soft-wrap (`wrap: true`) — the standard textarea shape. Without
/// this wiring every textarea would render with whatever the backend
/// hard-codes, ignoring the primitive's intent.
#[test]
fn text_area_defaults_to_wrap() {
    let rt = TestRuntime::new();
    let value: Signal<String> = signal!(String::new());
    let _owner = rt.render(text_area(value, |_: String| {}).into());
    rt.backend()
        .assert_any(|e| matches!(e, Event::CreateTextArea { wrap: true, .. }));
}

/// `code_mode()` (alias `wrap(false)`) flips the flag for the
/// unwrapped, horizontally-scrolling code-editor shape. Regression:
/// the fiddle editor relies on this reaching the backend — a wrapping
/// code editor would misalign against its syntax-highlight overlay.
#[test]
fn text_area_code_mode_disables_wrap() {
    let rt = TestRuntime::new();
    let value: Signal<String> = signal!(String::new());
    let _owner = rt.render(text_area(value, |_: String| {}).code_mode().into());
    rt.backend()
        .assert_any(|e| matches!(e, Event::CreateTextArea { wrap: false, .. }));
}

/// Omitting `on_key_down` leaves the create event recording
/// `has_key_handler: false` and registers no handler — so
/// `fire_key_event` returns `None`.
#[test]
fn no_on_key_down_means_no_handler_registered() {
    let rt = TestRuntime::new();
    let value: Signal<String> = signal!(String::new());
    let on_change = move |_: String| {};

    let _owner = rt.render(text_input(value, on_change).into());

    rt.backend().assert_any(|e| {
        matches!(
            e,
            Event::CreateTextInput {
                has_key_handler: false,
                ..
            }
        )
    });

    let outcome = rt
        .backend()
        .fire_key_event(NodeId(0), &make_key_event("Tab"));
    assert_eq!(outcome, None);
}
