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

mod common;

use common::TestRuntime;
use runtime_core::primitives::text_input::BlurOutcome;
use runtime_core::{text_input, IntoElement, Signal};

#[test]
fn on_blur_keep_threads_to_backend() {
    let rt = TestRuntime::new();
    let q = Signal::new(String::new());
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
    let q = Signal::new(String::new());
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
    let q = Signal::new(String::new());
    let _owner = rt.render(text_input(q, |_| {}).into_element());

    let core = rt.backend().inspector();
    assert!(
        core.blur_handlers.borrow().is_empty(),
        "a text_input without .on_blur() must not register a blur handler"
    );
}
