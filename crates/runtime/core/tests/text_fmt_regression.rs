//! Regression tests for the `text_fmt! + bind!` reactive-text path.
//!
//! The bug: `text_fmt!("Count: {}", bind!(count))` produces a
//! `TextSource::JsBinding(JsBindingSpec)`. The walker takes the
//! JS-binding fast path on web. The web backend's
//! `register_reactive_text_binding` previously registered the binding
//! on the JS side but never registered a `signal_js_notifier` for the
//! signal IDs — so `count.set/update` never reached the JS-side
//! dispatcher and the text never updated.
//!
//! Fix: per-signal `stringifiers: Vec<Rc<dyn Fn() -> String>>` now
//! flow from the macro through `JsBindingSpec` to
//! `Backend::register_reactive_text_binding`. The web backend uses
//! them to auto-install per-signal JS notifiers at bind time. These
//! tests pin the contract on the shape that survives the FFI bridge
//! (Tests 1, 5) — the wasm-side DOM tests live under
//! `crates/backend/web/tests/`.

#[path = "common/mod.rs"]
mod common;

use runtime_core::{signal, text, JsBindingSpec, Signal, TextSource};
use std::cell::Cell;
use std::rc::Rc;

use common::{Event, TestRuntime};

/// `text_fmt!("a={} b={}", bind!(s1), bind!(s2))` — assert the macro
/// emits one `Fn() -> String` stringifier per `bind!` slot AND that
/// each stringifier returns the matching signal's current value.
///
/// Catches: macro emits the wrong arity (e.g. forgets one stringifier
/// per signal, or shares a single closure across slots). The web
/// backend's per-signal auto-register loop assumes
/// `stringifiers.len() == signal_ids.len()` and would panic in debug
/// or silently install N copies of the same closure in release.
#[test]
fn regression_text_fmt_macro_emits_stringifier_per_signal() {
    let s1: Signal<u32> = signal!(7);
    let s2: Signal<u32> = signal!(42);

    let src = runtime_core::text_fmt!("a={} b={}", bind!(s1), bind!(s2));

    let spec = match src {
        TextSource::JsBinding(spec) => spec,
        _ => panic!("text_fmt! must produce TextSource::JsBinding"),
    };

    // Parallel-array invariant: one stringifier per signal slot.
    assert_eq!(spec.signal_ids.len(), 2);
    assert_eq!(spec.stringifiers.len(), 2, "one stringifier per bind! slot");
    assert_eq!(spec.initial_values, vec!["7".to_string(), "42".to_string()]);

    // Each stringifier reads the matching signal and Display-formats
    // it — same shape the JS dispatcher will Display-format the value
    // it receives across the FFI bridge.
    assert_eq!((spec.stringifiers[0])(), "7");
    assert_eq!((spec.stringifiers[1])(), "42");

    // And the stringifier tracks the live signal value — proves the
    // closure captured the Signal handle (not a snapshot of the
    // initial value).
    s1.set(99);
    s2.set(101);
    assert_eq!((spec.stringifiers[0])(), "99");
    assert_eq!((spec.stringifiers[1])(), "101");
}

/// `text_fmt!` with zero signal args (a captured-only template) still
/// produces a `JsBindingSpec` with an empty `stringifiers` slice. The
/// auto-register loop iterates `signal_ids.iter().zip(stringifiers)`
/// — a length mismatch here would either panic in debug or stop
/// short of registering the rightmost notifier. Covers the boundary.
#[test]
fn regression_text_fmt_no_signals_emits_empty_stringifiers() {
    let captured: u32 = 5;
    let src = runtime_core::text_fmt!("only={}", captured);
    let spec = match src {
        TextSource::JsBinding(spec) => spec,
        _ => panic!("text_fmt! must produce TextSource::JsBinding"),
    };
    assert_eq!(spec.signal_ids.len(), 0);
    assert_eq!(spec.stringifiers.len(), 0);
    assert_eq!((spec.compute_fallback)(), "only=5");
}

/// Backend reports `supports_js_text_bindings() = false` — walker
/// must lower the binding to the Effect-based fallback that re-runs
/// `compute_fallback` on every signal change. The trait signature
/// change to `register_reactive_text_binding` (the new `stringifiers`
/// param) must not break this path: backends that don't override
/// the trait method never see the new parameter, and the walker
/// never calls `register_reactive_text_binding` for them.
///
/// Pinned with `MockBackend`, which returns
/// `supports_js_text_bindings() = false` (the trait default). After
/// mount, calling `signal.set(...)` must produce an `UpdateText`
/// event with the new value.
#[test]
fn regression_text_fmt_compute_fallback_path_still_works() {
    let rt = TestRuntime::new();
    let count: Signal<u32> = signal!(0);

    // `text_fmt!` constructs a `TextSource::JsBinding` regardless of
    // backend — the variant is fixed at the call site. The walker
    // is where the per-backend gate lives.
    let _owner = rt.render(text(runtime_core::text_fmt!("Count: {}", bind!(count))).into());

    // Initial mount: `create_text("")` (the placeholder the walker
    // installs before the Effect first fires), then the Effect runs
    // and produces "Count: 0" via update_text.
    let events_after_mount = rt.events();
    assert!(
        events_after_mount
            .iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "Count: 0")),
        "expected initial UpdateText 'Count: 0' from the fallback Effect, got: {:#?}",
        events_after_mount,
    );

    // Fire a signal change — fallback Effect should re-run and
    // produce a new UpdateText.
    rt.backend_mut().clear_events();
    count.set(42);

    let post_set = rt.events();
    assert!(
        post_set
            .iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "Count: 42")),
        "expected UpdateText 'Count: 42' after signal.set(42), got: {:#?}",
        post_set,
    );
}

/// Direct construction of a `JsBindingSpec` (the manual form that
/// `text_fmt!` desugars to) still type-checks with the new
/// `stringifiers` field. Documents the public-surface shape — if
/// someone removes the field accidentally this test fails at compile.
#[test]
fn regression_text_fmt_manual_jsbinding_spec_compiles() {
    let s: Signal<u32> = signal!(0);
    let _src: TextSource = TextSource::JsBinding(JsBindingSpec {
        signal_ids: vec![s.id()],
        template_parts: vec!["v=".into(), "".into()],
        initial_values: vec!["0".into()],
        compute_fallback: Rc::new(move || format!("v={}", s.get())),
        stringifiers: vec![Rc::new(move || format!("{}", s.get()))],
    });
    // Just constructing it is the assertion — the test fails to
    // compile if the struct shape regresses.
    let _ = Cell::new(0u32); // keep the import live if assert helpers go away
}
