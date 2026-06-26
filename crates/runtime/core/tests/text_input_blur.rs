//! `on_blur` cancelable-blur plumbing.
//!
//! Verifies the framework path that the per-platform wiring relies on: the
//! `.on_blur(...)` builder stores a handler on `Element::TextInput`, the walker
//! threads it into `Backend::create_text_input`, and invoking it yields the
//! author's `BlurOutcome` (through the `cycle` batching wrapper).
//!
//! The native veto mechanisms themselves (iOS `textFieldShouldEndEditing:`,
//! macOS `FlippedView` outside-click, web refocus) are platform UI behavior and
//! aren't reachable from a host test — but they all consult exactly this
//! handler, so proving it threads end-to-end pins the contract they share.

// Include only the harness pieces we need, NOT the whole `common` module —
// `common/counted.rs` currently fails to compile against the now-`pub(crate)`
// `Effect::new` (a separate, pre-existing harness breakage), which would
// otherwise block this file. `mock_backend` is self-contained and `runtime`
// only needs `super::mock_backend`, so both resolve as crate-root siblings.
#[path = "common/mock_backend.rs"]
mod mock_backend;
#[path = "common/runtime.rs"]
mod runtime;

use runtime::TestRuntime;
use runtime_core::primitives::text_input::BlurOutcome;
use runtime_core::{signal, text_input, IntoElement};

#[test]
fn on_blur_keep_threads_to_backend() {
    let rt = TestRuntime::new();
    let q = signal!(String::new());
    let _owner =
        rt.render(text_input(q, |_| {}).on_blur(|| BlurOutcome::Keep).into_element());

    let core = rt.backend().inspector();
    let ids: Vec<_> = core.blur_handlers.borrow().keys().copied().collect();
    assert_eq!(ids.len(), 1, "exactly one on_blur handler should register");

    // Invoking the registered handler returns the author's veto decision.
    assert_eq!(rt.backend().fire_blur(ids[0]), Some(BlurOutcome::Keep));
}

#[test]
fn on_blur_allow_threads_to_backend() {
    let rt = TestRuntime::new();
    let q = signal!(String::new());
    let _owner =
        rt.render(text_input(q, |_| {}).on_blur(|| BlurOutcome::Allow).into_element());

    let core = rt.backend().inspector();
    let id = *core
        .blur_handlers
        .borrow()
        .keys()
        .next()
        .expect("on_blur handler registered");
    assert_eq!(rt.backend().fire_blur(id), Some(BlurOutcome::Allow));
}

#[test]
fn no_on_blur_registers_no_handler() {
    let rt = TestRuntime::new();
    let q = signal!(String::new());
    let _owner = rt.render(text_input(q, |_| {}).into_element());

    let core = rt.backend().inspector();
    assert!(
        core.blur_handlers.borrow().is_empty(),
        "a text_input without .on_blur() must not register a blur handler"
    );
}
