//! Native reactive-parity regression suite.
//!
//! User feedback (device-only, web-clean): two reactive idioms that
//! work on web were reported as NOT updating on native (iOS/Android):
//!
//!   1. `view(...).with_style(move || ...)` — reactive style closure
//!   2. `text(move || ...)`                 — reactive text content
//!
//! …while the *same* `pressable(...).with_style(move || ...)` pattern
//! DID re-render on native. The feedback framed it as a primitive-level
//! parity gap.
//!
//! These tests pin the framework-core contract for all three idioms,
//! exercised through the real build walker against `MockBackend`, which
//! reports the **native** trait shape (`handles_states_natively ==
//! false`, no `create_text_with_id`, no JS class/text bindings). A
//! signal write must re-fire the bound `Effect` and reach the backend:
//! `apply_style` for the two style cases, `update_text` for the text
//! case.
//!
//! If these pass, the core + walker path is sound under native
//! semantics and any remaining device-only gap lives in a specific
//! backend crate's `apply_style` / `update_text` (or its event→signal
//! flush), not here. They are the floor the per-backend investigation
//! builds on, and they guard core from a regression that would silently
//! freeze every reactive `view`/`text` on mobile.

#[path = "common/mod.rs"]
mod common;

use common::{Event, TestRuntime};
use runtime_core::{
    signal, text, view, Color, IntoElement, Signal, StyleApplication, StyleRules, StyleSheet,
    Tokenized, VariantSet,
};
use std::rc::Rc;

/// Two single-rule sheets distinguished by background color, so a
/// reactive closure can swap between them on a signal flip.
fn sheet_with_bg(hex: &'static str) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(move |_vs: &VariantSet| StyleRules {
        background: Some(Tokenized::Literal(Color(hex.into()))),
        ..Default::default()
    }))
}

fn apply_style_count(rt: &TestRuntime) -> usize {
    rt.events()
        .iter()
        .filter(|e| matches!(e, Event::ApplyStyle { .. }))
        .count()
}

/// `view(...).with_style(move || ...)` — opaque reactive style closure
/// (`StyleSource::Reactive`). On a signal flip the bound style `Effect`
/// must re-fire and re-apply through `apply_style` on a native-shaped
/// backend.
#[test]
fn regression_native_reactive_view_with_style_reapplies_on_signal() {
    let rt = TestRuntime::new();
    let on = sheet_with_bg("#aaa");
    let off = sheet_with_bg("#bbb");
    let ranked: Signal<bool> = signal!(false);

    let on_c = on.clone();
    let off_c = off.clone();
    let _owner = rt.render(
        view(vec![])
            .with_style(move || {
                StyleApplication::new(if ranked.get() { on_c.clone() } else { off_c.clone() })
            })
            .into_element(),
    );

    assert!(
        apply_style_count(&rt) >= 1,
        "reactive view style must apply once at mount, got: {:#?}",
        rt.events()
    );

    rt.backend_mut().clear_events();
    ranked.set(true);
    assert_eq!(
        apply_style_count(&rt),
        1,
        "view().with_style(closure) must re-apply exactly once on signal flip \
         (native backend shape), got: {:#?}",
        rt.events()
    );
}

/// `pressable(...).with_style(move || ...)` — the idiom the feedback
/// said *did* work on native. Pinned here so the comparison against
/// `view` is apples-to-apples and a future change can't silently
/// regress the one primitive that worked.
#[test]
fn regression_native_reactive_pressable_with_style_reapplies_on_signal() {
    let rt = TestRuntime::new();
    let on = sheet_with_bg("#aaa");
    let off = sheet_with_bg("#bbb");
    let selected: Signal<bool> = signal!(false);

    let on_c = on.clone();
    let off_c = off.clone();
    let _owner = rt.render(
        pressable_with_reactive_style(selected, on_c, off_c),
    );

    assert!(
        apply_style_count(&rt) >= 1,
        "reactive pressable style must apply once at mount, got: {:#?}",
        rt.events()
    );

    rt.backend_mut().clear_events();
    selected.set(true);
    assert_eq!(
        apply_style_count(&rt),
        1,
        "pressable().with_style(closure) must re-apply exactly once on signal flip, got: {:#?}",
        rt.events()
    );
}

fn pressable_with_reactive_style(
    selected: Signal<bool>,
    on: Rc<StyleSheet>,
    off: Rc<StyleSheet>,
) -> runtime_core::Element {
    runtime_core::pressable(vec![], || {})
        .with_style(move || {
            StyleApplication::new(if selected.get() { on.clone() } else { off.clone() })
        })
        .into_element()
}

/// `text(move || ...)` — opaque reactive text content
/// (`TextSource::Bound`, no JS binding). On a signal flip the bound
/// content `Effect` must re-fire and push the new string through
/// `update_text` on a native-shaped backend (which returns `None` from
/// `create_text_with_id`, so the per-node `update_text` path is taken).
#[test]
fn regression_native_reactive_text_content_updates_on_signal() {
    let rt = TestRuntime::new();
    let pos: Signal<Option<usize>> = signal!(None);

    let _owner = rt.render(
        text(move || match pos.get() {
            Some(i) => format!("{}", i + 1),
            None => String::new(),
        })
        .into_element(),
    );

    // Mount: closure evaluates to "" (pos == None). The walker creates
    // the text node with a placeholder and the bound Effect runs once.
    rt.backend_mut().clear_events();
    pos.set(Some(0));
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateText { content, .. } if content == "1")),
        "text(closure) must update_text to '1' on signal flip (native backend shape), got: {:#?}",
        rt.events()
    );
}
