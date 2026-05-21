//! Walker control-flow coverage: `switch` with multiple arms, the
//! non-batched `Repeat` path, `Bound`-primitive lifecycle, and
//! per-row style attach.
//!
//! Complements:
//! - `lifecycle.rs` — focuses on `when` flip + plain owner drop
//! - `rebuild.rs` — focuses on the `setRows` bench pattern (one
//!   Switch arm + signal-driven Repeat count)
//! - `batched_repeat.rs` — focuses on the `supports_batched_repeat`
//!   fast path
//!
//! This file fills the gaps those don't cover:
//! - Switch with 3+ distinct discriminant values
//! - Non-batched Repeat (default mock config: no opt-in)
//! - `apply_style` fires through static-style attach on plain Views

use std::rc::Rc;

use framework_core::{
    signal, switch, text, view, when, IntoPrimitive, Primitive, Signal, StyleApplication,
    StyleRules, StyleSheet, VariantSet,
};

use crate::common::{Event, TestRuntime};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_static_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules::default()))
}

fn count_create_text(events: &[Event], needle: &str) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::CreateText { content } if content == needle))
        .count()
}

fn any_event_matches(events: &[Event], pred: impl Fn(&Event) -> bool) -> bool {
    events.iter().any(pred)
}

// ---------------------------------------------------------------------------
// Switch — multiple arms
// ---------------------------------------------------------------------------

/// Three discriminant values, three distinct arm bodies. Each value
/// must mount the matching arm and only that arm.
#[test]
fn switch_with_three_arms_mounts_the_matching_branch() {
    let rt = TestRuntime::new();
    let mode: Signal<u32> = signal!(0u32);

    let tree = switch(
        move || mode.get(),
        move |m| match m {
            0 => text("alpha").into_primitive(),
            1 => text("beta").into_primitive(),
            2 => text("gamma").into_primitive(),
            _ => text("default").into_primitive(),
        },
    );
    let _owner = rt.render(tree);

    // Initial: only alpha.
    let initial = rt.events();
    assert_eq!(count_create_text(&initial, "alpha"), 1);
    assert_eq!(count_create_text(&initial, "beta"), 0);
    assert_eq!(count_create_text(&initial, "gamma"), 0);

    // Flip to 1: beta mounts.
    rt.backend_mut().clear_events();
    mode.set(1);
    let after_1 = rt.events();
    assert_eq!(count_create_text(&after_1, "beta"), 1);
    assert_eq!(count_create_text(&after_1, "alpha"), 0);
    assert_eq!(count_create_text(&after_1, "gamma"), 0);

    // Flip to 2: gamma mounts.
    rt.backend_mut().clear_events();
    mode.set(2);
    let after_2 = rt.events();
    assert_eq!(count_create_text(&after_2, "gamma"), 1);
}

/// A discriminant value not listed in any specific arm falls through
/// to the default. Catches "we lost the wildcard arm".
#[test]
fn switch_default_arm_fires_for_unmatched_discriminant() {
    let rt = TestRuntime::new();
    let mode: Signal<u32> = signal!(99u32);

    let tree = switch(
        move || mode.get(),
        move |m| match m {
            0 => text("zero").into_primitive(),
            1 => text("one").into_primitive(),
            _ => text("fallback").into_primitive(),
        },
    );
    let _owner = rt.render(tree);

    let events = rt.events();
    assert_eq!(count_create_text(&events, "fallback"), 1);
    assert_eq!(count_create_text(&events, "zero"), 0);
    assert_eq!(count_create_text(&events, "one"), 0);
}

/// Switch with a non-`u32` discriminant. Tests the closure-driven
/// switch path that's polymorphic over any `PartialEq + 'static`.
#[test]
fn switch_with_string_discriminant_routes_correctly() {
    let rt = TestRuntime::new();
    let label: Signal<&'static str> = signal!("home");

    let tree = switch(
        move || label.get(),
        |s: &&'static str| match *s {
            "home" => text("home-page").into_primitive(),
            "about" => text("about-page").into_primitive(),
            _ => text("404").into_primitive(),
        },
    );
    let _owner = rt.render(tree);

    assert!(any_event_matches(&rt.events(), |e| matches!(
        e,
        Event::CreateText { content } if content == "home-page"
    )));

    rt.backend_mut().clear_events();
    label.set("about");
    assert!(any_event_matches(&rt.events(), |e| matches!(
        e,
        Event::CreateText { content } if content == "about-page"
    )));
}

// ---------------------------------------------------------------------------
// Repeat — non-batched (per-call) path
// ---------------------------------------------------------------------------

/// With the default mock config (`supports_batched_repeat = false`),
/// Repeat must take the per-call path: each row gets its own
/// CreateView / CreateText / Insert events, and the final
/// parent-attach is via `insert_many`.
#[test]
fn repeat_non_batched_path_emits_per_row_events_and_insert_many() {
    let rt = TestRuntime::new();
    let row_builder: Box<dyn Fn(usize) -> Primitive> = Box::new(|i| {
        view(vec![text(format!("Row #{}", i)).into_primitive()]).into_primitive()
    });
    let tree = view(vec![Primitive::Repeat {
        count: 4,
        row_builder,
    }])
    .into_primitive();
    let _owner = rt.render(tree);

    let events = rt.events();
    // 4 row Views + 4 row Texts + 1 outer container View = 9 creates.
    let view_creates = events
        .iter()
        .filter(|e| matches!(e, Event::CreateView))
        .count();
    let text_creates = events
        .iter()
        .filter(|e| matches!(e, Event::CreateText { .. }))
        .count();
    assert_eq!(view_creates, 5, "1 outer + 4 row views");
    assert_eq!(text_creates, 4, "1 text per row");

    // Texts appear in iteration order.
    let text_contents: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            Event::CreateText { content } => Some(content.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        text_contents,
        vec!["Row #0", "Row #1", "Row #2", "Row #3"],
    );

    // Final attach goes through insert_many with all 4 row tops.
    let insert_manys: Vec<usize> = events
        .iter()
        .filter_map(|e| match e {
            Event::InsertMany { children, .. } => Some(children.len()),
            _ => None,
        })
        .collect();
    assert!(
        insert_manys.contains(&4),
        "expected an InsertMany with 4 children; got {:?}",
        insert_manys,
    );
}

/// Empty Repeat: `count = 0` produces no row events and no
/// insert_many (the batched-Repeat path returns true early; the
/// non-batched path's eager loop runs zero times and then calls
/// insert_many with an empty Vec — backends typically no-op that).
#[test]
fn repeat_with_zero_count_produces_no_row_events() {
    let rt = TestRuntime::new();
    let row_builder: Box<dyn Fn(usize) -> Primitive> = Box::new(|i| {
        text(format!("Row #{}", i)).into_primitive()
    });
    let tree = view(vec![Primitive::Repeat {
        count: 0,
        row_builder,
    }])
    .into_primitive();
    let _owner = rt.render(tree);

    let events = rt.events();
    // No row texts.
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::CreateText { content } if content.starts_with("Row #")))
            .count(),
        0,
    );
    // Outer view still mounted.
    assert!(events.iter().any(|e| matches!(e, Event::CreateView)));
}

// ---------------------------------------------------------------------------
// Style attach on the non-batched path
// ---------------------------------------------------------------------------

/// A Repeat row with a static style, on the default (non-batched)
/// mock config: the walker takes the per-call path, and apply_style
/// fires once per styled row. Verifies the style attachment
/// machinery works outside the batched-Repeat fast path.
#[test]
fn static_styled_row_in_non_batched_repeat_fires_apply_style() {
    let rt = TestRuntime::new();
    let sheet = make_static_sheet();
    let sheet_for_rows = sheet;
    let row_builder: Box<dyn Fn(usize) -> Primitive> = Box::new(move |i| {
        view(vec![text(format!("Row #{}", i)).into_primitive()])
            .with_style(sheet_for_rows.clone())
            .into_primitive()
    });
    let tree = view(vec![Primitive::Repeat {
        count: 3,
        row_builder,
    }])
    .into_primitive();
    let _owner = rt.render(tree);

    let events = rt.events();
    let apply_count = events
        .iter()
        .filter(|e| matches!(e, Event::ApplyStyle { .. }))
        .count();
    assert_eq!(
        apply_count, 3,
        "expected one ApplyStyle per row, got {} (events: {:#?})",
        apply_count, events,
    );

    // Stylesheet registration fires once (or once per dedup key —
    // either way, at least once).
    let register_count = events
        .iter()
        .filter(|e| matches!(e, Event::RegisterStylesheet { .. }))
        .count();
    assert!(
        register_count >= 1,
        "expected at least one RegisterStylesheet event",
    );
}

// ---------------------------------------------------------------------------
// Bound primitive — style + render through the builder layer
// ---------------------------------------------------------------------------

/// The `Bound<H>` wrapper that `view()` / `text()` etc. return must
/// participate in the same mount path as a bare Primitive. This
/// catches a regression where a builder method silently dropped the
/// underlying primitive.
#[test]
fn bound_view_with_style_mounts_and_fires_apply_style() {
    let rt = TestRuntime::new();
    let sheet = make_static_sheet();
    let bound = view(vec![text("inside").into_primitive()]).with_style(sheet);
    let _owner = rt.render(bound.into_primitive());

    let events = rt.events();
    assert!(events.iter().any(|e| matches!(e, Event::CreateView)));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::CreateText { content } if content == "inside")));
    assert!(
        events.iter().any(|e| matches!(e, Event::ApplyStyle { .. })),
        "Bound::with_style must route to the static-style apply path",
    );
}

/// `Bound::with_style` accepts a closure form, which yields a
/// `StyleSource::Reactive`. That should still mount the node, but
/// apply happens through the reactive (Effect-driven) path. Verify
/// the node mounts and at least one ApplyStyle fires.
#[test]
fn bound_view_with_reactive_style_still_mounts_and_applies() {
    let rt = TestRuntime::new();
    let sheet = make_static_sheet();
    let sheet_for_closure = sheet;
    let bound = view(vec![text("reactive").into_primitive()])
        .with_style(move || StyleApplication::new(sheet_for_closure.clone()));
    let _owner = rt.render(bound.into_primitive());

    let events = rt.events();
    assert!(events.iter().any(|e| matches!(e, Event::CreateView)));
    assert!(
        events.iter().any(|e| matches!(e, Event::ApplyStyle { .. })),
        "reactive-style path must still result in ApplyStyle firing on mount",
    );
}

// ---------------------------------------------------------------------------
// Nested control flow
// ---------------------------------------------------------------------------

/// A `when` inside a `switch` arm: switching arms must tear down the
/// inner `when`-scope cleanly, then a fresh `when` mounts on the
/// new arm body.
#[test]
fn nested_when_inside_switch_arm_rebuilds_cleanly_on_arm_swap() {
    let rt = TestRuntime::new();
    let outer: Signal<u32> = signal!(0u32);
    let inner: Signal<bool> = signal!(true);

    let tree = switch(
        move || outer.get(),
        move |m| match m {
            0 => when(
                move || inner.get(),
                || text("a-then").into_primitive(),
                || text("a-else").into_primitive(),
            ),
            _ => text("b").into_primitive(),
        },
    );
    let _owner = rt.render(tree);

    // Initial: outer=0, inner=true → "a-then".
    let initial = rt.events();
    assert_eq!(count_create_text(&initial, "a-then"), 1);
    assert_eq!(count_create_text(&initial, "a-else"), 0);
    assert_eq!(count_create_text(&initial, "b"), 0);

    // Flip the OUTER discriminant first — we should land on "b".
    rt.backend_mut().clear_events();
    outer.set(1);
    let after_swap = rt.events();
    assert_eq!(count_create_text(&after_swap, "b"), 1);

    // Flip the INNER signal now. Outer arm is no longer the
    // when-branch, so flipping `inner` must NOT mount any "a-*"
    // — the inner Effect must have been dropped with the arm.
    rt.backend_mut().clear_events();
    inner.set(false);
    let after_inner = rt.events();
    assert_eq!(
        count_create_text(&after_inner, "a-then"),
        0,
        "old arm's reactive scope should have been torn down; events: {:#?}",
        after_inner,
    );
    assert_eq!(count_create_text(&after_inner, "a-else"), 0);
}

/// Going back to the original outer arm rebuilds the inner `when`
/// from scratch — it must mount once for the current inner-signal
/// state.
#[test]
fn switching_back_to_arm_with_nested_when_rebuilds_inner() {
    let rt = TestRuntime::new();
    let outer: Signal<u32> = signal!(0u32);
    let inner: Signal<bool> = signal!(true);

    let tree = switch(
        move || outer.get(),
        move |m| match m {
            0 => when(
                move || inner.get(),
                || text("a-then").into_primitive(),
                || text("a-else").into_primitive(),
            ),
            _ => text("b").into_primitive(),
        },
    );
    let _owner = rt.render(tree);
    rt.backend_mut().clear_events();

    // Flip outer → flip inner (while outer=1, no effect) → flip
    // outer back to 0. The arm should re-mount with the CURRENT
    // inner state (false → "a-else").
    outer.set(1);
    inner.set(false);
    rt.backend_mut().clear_events();
    outer.set(0);

    let after = rt.events();
    assert_eq!(
        count_create_text(&after, "a-else"),
        1,
        "rebuilt arm must mount the else-branch (inner = false now); events: {:#?}",
        after,
    );
    assert_eq!(count_create_text(&after, "a-then"), 0);
}
