//! Reactive `secure` plumbing — a live `secure` source toggles the native
//! mask in place (`Backend::update_text_input_secure`) WITHOUT rebuilding the
//! input, while a `Static` mask installs no effect at all.
//!
//! This pins the framework path the password show/hide pattern relies on: the
//! controlled `value` signal carries the typed text across a mask toggle
//! because the input is never recreated. The native secure-entry mechanisms
//! themselves (web input type, UIKit `isSecureTextEntry`, AppKit secure cell)
//! aren't reachable from a host test, but they all hang off exactly this
//! `update_text_input_secure` call, so proving it threads end-to-end pins the
//! contract they share.

// Include only the harness pieces we need, NOT the whole `common` module —
// `common/counted.rs` currently fails to compile against the now-`pub(crate)`
// `Effect::new` (a separate, pre-existing harness breakage). `mock_backend` is
// self-contained and `runtime` only needs `super::mock_backend`.
#[path = "common/mock_backend.rs"]
mod mock_backend;
#[path = "common/runtime.rs"]
mod runtime;

use mock_backend::Event;
use runtime::TestRuntime;
use runtime_core::{rx, signal, text_input, IntoElement, Signal};

#[test]
fn reactive_secure_toggles_in_place_without_rebuild() {
    let rt = TestRuntime::new();
    let value = signal!(String::new());
    let visible: Signal<bool> = signal!(false);

    // `secure = !visible`: masked while hidden, revealed while visible.
    let tree = text_input(value, |_| {})
        .secure(rx!(!visible.get()))
        .into_element();
    let _owner = rt.render(tree);

    // Born masked (visible=false → secure=true).
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::CreateTextInput { secure: true, .. })),
        "input is created masked when visible=false: {:?}",
        rt.events()
    );

    // Reveal → the mask must toggle via an in-place update, never a rebuild.
    rt.backend_mut().clear_events();
    visible.set(true);
    let evs = rt.events();
    assert!(
        evs.iter()
            .any(|e| matches!(e, Event::UpdateTextInputSecure { secure: false, .. })),
        "revealing must push update_text_input_secure(false): {:?}",
        evs
    );
    assert!(
        !evs.iter().any(|e| matches!(e, Event::CreateTextInput { .. })),
        "the input must NOT be rebuilt — no new CreateTextInput: {:?}",
        evs
    );

    // Hide again → masked once more, same in-place path.
    rt.backend_mut().clear_events();
    visible.set(false);
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateTextInputSecure { secure: true, .. })),
        "hiding must push update_text_input_secure(true): {:?}",
        rt.events()
    );
}

#[test]
fn static_secure_installs_no_toggle_effect() {
    let rt = TestRuntime::new();
    let value = signal!(String::new());
    // A bare `bool` is a `Static` mask: threaded to create, but no effect.
    let _owner = rt.render(text_input(value, |_| {}).secure(true).into_element());

    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::CreateTextInput { secure: true, .. })),
        "static secure=true threads to create_text_input"
    );
    assert!(
        !rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateTextInputSecure { .. })),
        "a Static mask installs no effect, so update_text_input_secure never fires: {:?}",
        rt.events()
    );
}

#[test]
fn reactive_placeholder_updates_in_place_without_rebuild() {
    let rt = TestRuntime::new();
    let value = signal!(String::new());
    let hint: Signal<bool> = signal!(false);

    let tree = text_input(value, |_| {})
        .placeholder_reactive(rx!(if hint.get() {
            Some("Required".to_string())
        } else {
            None
        }))
        .into_element();
    let _owner = rt.render(tree);

    rt.backend_mut().clear_events();
    hint.set(true);
    let evs = rt.events();
    assert!(
        evs.iter().any(|e| matches!(
            e,
            Event::UpdateTextInputPlaceholder { placeholder: Some(p), .. } if p == "Required"
        )),
        "flipping the placeholder source must push update_text_input_placeholder in place: {:?}",
        evs
    );
    assert!(
        !evs.iter().any(|e| matches!(e, Event::CreateTextInput { .. })),
        "the input must NOT be rebuilt: {:?}",
        evs
    );
}
