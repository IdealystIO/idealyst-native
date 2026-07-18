//! F-string text interpolation (`text { "count: {count}" }`) — the
//! typed-slot path added in 0.3.
//!
//! A text literal with `{name}` placeholders lowers to a
//! `TextSlotPart` list assembled by `__idealyst_text_from_parts`; each
//! slot classifies by the interpolated value's TYPE through the
//! `StaticTextSlot`/`ReactiveTextSlot` method dispatch (the text
//! analog of `StaticCond`/`ReactiveCond` for `if`):
//!
//! - all slots static → `TextSource::Static` (zero reactive machinery),
//! - signal slots → `TextSource::JsBinding` (web fast path; Effect
//!   fallback on other backends — that's the path the mock backend
//!   exercises here),
//! - any id-less reactive slot (`Reactive::Dynamic` props) →
//!   `TextSource::Bound` via the Effect path.

#[path = "common/mod.rs"]
mod common;

use runtime_core::{
    signal, text, ui, Element, Reactive, ReactiveTextSlot, Signal, StaticTextSlot, TextSlot,
    TextSlotPart, TextSource, __idealyst_text_from_parts,
};
use std::rc::Rc;

use common::{Event, TestRuntime};

/// All-static interpolation bakes to a `Static` source at build time:
/// the mock backend sees the final string in `CreateText` and NO
/// `UpdateText` (no Effect, no binding — same cost as a plain literal).
#[test]
fn fstring_all_static_bakes_to_static_text() {
    let rt = TestRuntime::new();
    let name = "Ada".to_string();
    let tree: Element = ui! { text { "hi {name}!" } };
    let _owner = rt.render(tree);
    let events = rt.events();
    assert!(
        events.iter().any(|e| matches!(e, Event::CreateText { content } if content == "hi Ada!")),
        "static f-string must mount with the final string: {events:#?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, Event::UpdateText { .. })),
        "static f-string must not install any reactive update path: {events:#?}"
    );
}

/// A signal slot is LIVE by type — no closure, no `.get()` at the call
/// site. On a backend without JS bindings the compute fallback runs in
/// an Effect: initial paint, then repaint on `set`.
#[test]
fn fstring_signal_slot_updates_on_set() {
    let rt = TestRuntime::new();
    let count: Signal<u32> = signal(0);
    let tree: Element = ui! { text { "count: {count}" } };
    let _owner = rt.render(tree);
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "count: 0")),
        "initial paint through the fallback Effect: {:#?}",
        rt.events()
    );

    rt.backend_mut().clear_events();
    count.set(42);
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "count: 42")),
        "signal set must repaint the interpolated text: {:#?}",
        rt.events()
    );
}

/// Mixed static + signal slots: the static value bakes into the
/// template, the signal slot stays live, and format specs pass through
/// to `format!`.
#[test]
fn fstring_mixed_slots_and_format_spec() {
    let rt = TestRuntime::new();
    let label = "ratio".to_string();
    let ratio: Signal<f64> = signal(0.5);
    let tree: Element = ui! { text { "{label}: {ratio:.2}" } };
    let _owner = rt.render(tree);
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "ratio: 0.50")),
        "{:#?}",
        rt.events()
    );

    rt.backend_mut().clear_events();
    ratio.set(0.789);
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "ratio: 0.79")),
        "spec must apply on every repaint: {:#?}",
        rt.events()
    );
}

/// The JsBinding structural contract for the web fast path: template
/// parts surround each signal slot, ids/initials/stringifiers are
/// parallel arrays — the invariants the web backend's binding layer assumes.
#[test]
fn fstring_signal_slots_build_jsbinding_shape() {
    let count: Signal<u32> = signal(3);
    let fmt = |v: &dyn std::fmt::Display| format!("{v}");
    let src = __idealyst_text_from_parts(vec![
        TextSlotPart::Lit("count: "),
        TextSlotPart::Slot(count.__idealyst_text_slot(fmt)),
    ]);
    let spec = match src {
        TextSource::JsBinding(spec) => spec,
        _ => panic!("signal-only slots must build the JsBinding fast path"),
    };
    assert_eq!(spec.signal_ids, vec![count.id()]);
    assert_eq!(spec.template_parts, vec!["count: ".to_string(), String::new()]);
    assert_eq!(spec.initial_values, vec!["3".to_string()]);
    assert_eq!(spec.stringifiers.len(), 1);
    assert_eq!((spec.stringifiers[0])(), "3");
    assert_eq!((spec.compute_fallback)(), "count: 3");
    count.set(9);
    assert_eq!((spec.stringifiers[0])(), "9");
    assert_eq!((spec.compute_fallback)(), "count: 9");
}

/// `Reactive<T>` props interpolate: the `Static` variant bakes in; the
/// `Dynamic` variant (no signal id) forces the `Bound`/Effect path and
/// stays live.
#[test]
fn fstring_reactive_prop_slots() {
    let fmt = |v: &dyn std::fmt::Display| format!("{v}");
    // Static prop → static slot → Static source.
    let stat: Reactive<i32> = Reactive::Static(7);
    let src = __idealyst_text_from_parts(vec![
        TextSlotPart::Lit("n="),
        TextSlotPart::Slot(stat.__idealyst_text_slot(fmt)),
    ]);
    assert!(
        matches!(&src, TextSource::Static(s) if s == "n=7"),
        "Static prop slot must not create reactive machinery"
    );

    // Dynamic prop → Computed slot → Bound source, live via the Effect.
    let rt = TestRuntime::new();
    let count: Signal<u32> = signal(1);
    let dynamic: Reactive<u32> = Reactive::Dynamic(Rc::new(move || count.get()));
    let src = __idealyst_text_from_parts(vec![
        TextSlotPart::Lit("n="),
        TextSlotPart::Slot(dynamic.__idealyst_text_slot(fmt)),
    ]);
    assert!(matches!(&src, TextSource::Bound(_)), "id-less slot must route to Bound");
    let _owner = rt.render(text(src).into());
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "n=1")),
        "{:#?}",
        rt.events()
    );
    rt.backend_mut().clear_events();
    count.set(8);
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "n=8")),
        "Dynamic prop slot must stay live: {:#?}",
        rt.events()
    );
}

/// A memo output (`ReadSignal`) interpolates live — the everyday
/// derived-value case from the docs (`doubled`, `is_high`).
#[test]
fn fstring_read_signal_slot_is_live() {
    let rt = TestRuntime::new();
    let count: Signal<i32> = signal(2);
    let doubled = runtime_core::memo(move || count.get() * 2);
    let tree: Element = ui! { text { "doubled: {doubled}" } };
    let _owner = rt.render(tree);
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "doubled: 4")),
        "{:#?}",
        rt.events()
    );
    rt.backend_mut().clear_events();
    count.set(5);
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "doubled: 10")),
        "memo slot must repaint on upstream change: {:#?}",
        rt.events()
    );
}

/// Two live slots: parallel-array arity (one stringifier per signal,
/// N+1 template parts) and per-slot value tracking. Inherited from the
/// `text_fmt!` regression suite when that macro was removed — the web
/// backend's auto-register loop zips `signal_ids` with `stringifiers`
/// and a mismatch would stop short of registering the rightmost
/// notifier.
#[test]
fn fstring_two_live_slots_keep_parallel_arrays() {
    let s1: Signal<u32> = signal(7);
    let s2: Signal<u32> = signal(42);
    let fmt = |v: &dyn std::fmt::Display| format!("{v}");
    let src = __idealyst_text_from_parts(vec![
        TextSlotPart::Lit("a="),
        TextSlotPart::Slot(s1.__idealyst_text_slot(fmt)),
        TextSlotPart::Lit(" b="),
        TextSlotPart::Slot(s2.__idealyst_text_slot(fmt)),
    ]);
    let spec = match src {
        TextSource::JsBinding(spec) => spec,
        _ => panic!("live slots must build JsBinding"),
    };
    assert_eq!(spec.signal_ids.len(), 2);
    assert_eq!(spec.stringifiers.len(), 2, "one stringifier per live slot");
    assert_eq!(spec.template_parts.len(), 3, "N+1 template parts");
    assert_eq!(spec.initial_values, vec!["7".to_string(), "42".to_string()]);
    // Stringifiers capture the signal HANDLE, not a value snapshot.
    s1.set(99);
    s2.set(101);
    assert_eq!((spec.stringifiers[0])(), "99");
    assert_eq!((spec.stringifiers[1])(), "101");
    assert_eq!((spec.compute_fallback)(), "a=99 b=101");
}

/// Direct construction of a `JsBindingSpec` (the manual form the
/// f-string lowering desugars to) type-checks — pins the public struct
/// shape for backends and hand-written bindings.
#[test]
fn manual_jsbinding_spec_compiles() {
    let s: Signal<u32> = signal(0);
    let _src: TextSource = TextSource::JsBinding(runtime_core::JsBindingSpec {
        signal_ids: vec![s.id()],
        template_parts: vec!["v=".into(), "".into()],
        initial_values: vec!["0".into()],
        compute_fallback: Rc::new(move || format!("v={}", s.get())),
        stringifiers: vec![Rc::new(move || format!("{}", s.get()))],
    });
}

/// The `TextSlot` enum is public API for the dispatch traits — pin the
/// static-slot construction so the variant shape can't silently drift.
#[test]
fn fstring_static_slot_applies_format_once() {
    let v = 1.23456_f64;
    let slot = v.__idealyst_text_slot(|d| format!("{d:.1}"));
    assert!(matches!(slot, TextSlot::Static(s) if s == "1.2"));
}
