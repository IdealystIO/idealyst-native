//! Walker coverage for `Element::When` / `Element::Switch` hydration
//! adoption — the regression guards for the
//! `[hydrate] SSR/client diverge — remounting just this subtree`
//! warning when the SSR tree contains a reactive control-flow region.
//!
//! Bug: the prior build path ALWAYS called `clear_children` on the
//! reactive anchor (Switch via a deferred microtask, When inside the
//! Effect's first synchronous fire) and rebuilt the arm from scratch.
//! Under hydration, the anchor already holds the SSR arm — the
//! `clear_children` wipes it, the rebuild fresh-mints elements, and
//! the hydration cursor's subsequent adoption mis-aligns (the next
//! sibling sees a stale node parked at the cursor).
//!
//! Fix (`walker/when_switch.rs`): when `Backend::is_hydrating()` is
//! `true`, build the arm INLINE during the synchronous walker pass
//! and skip `clear_children`. The Effect's first scheduled microtask
//! (Switch) / synchronous fire (When) is gated by `is_hydrating()`
//! and no-ops, so the SSR DOM the inline build adopted stays put.
//!
//! These tests use `MockBackend` with `MockBackendConfig::hydrating`
//! set, asserting that `Event::ClearChildren` does NOT fire for the
//! anchor under hydration. End-to-end DOM adoption against a real
//! browser SSR document is covered by the website `serve` example;
//! this layer pins the framework-core contract.

use runtime_core::{switch, text, view, when, Element};

use crate::common::{Event, MockBackendConfig, TestRuntime};

/// REGRESSION GUARD: rendering a `switch(...)` element through the
/// walker under hydration must NOT call `clear_children` on the
/// reactive anchor. The SSR DOM is still attached to it; clearing
/// would wipe the server-rendered arm and force a remount.
#[test]
fn switch_under_hydration_skips_clear_children() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        hydrating: true,
        ..MockBackendConfig::default()
    });

    // A single-arm switch: the discriminant reads no signals, so the
    // Effect fires exactly once. The walker's hydration branch is the
    // path under test.
    let elem = view(vec![switch(
        || 0u8,
        |k| match k {
            0 => text("HYDRATED-ARM").into(),
            _ => text("ELSE").into(),
        },
    )]);
    let _owner = rt.render(elem.into());

    let events = rt.events();
    assert!(
        !events.iter().any(|e| matches!(e, Event::ClearChildren { .. })),
        "switch must NOT clear_children under hydration — the SSR arm is \
         already attached to the anchor. events: {events:#?}"
    );
    // And the arm IS built: the walker descends through the anchor's
    // (SSR-equivalent) subtree and creates the arm's text node.
    assert!(
        events.iter().any(|e| matches!(e, Event::CreateText { content } if content == "HYDRATED-ARM")),
        "switch must still build the active arm under hydration; events: {events:#?}"
    );
}

/// REGRESSION GUARD: rendering a `when(...)` element through the
/// walker under hydration must NOT call `clear_children` on the
/// reactive anchor. Same shape as the switch guard — `When`'s build
/// path used to unconditionally clear+rebuild inside the Effect's
/// first synchronous fire.
#[test]
fn when_under_hydration_skips_clear_children() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        hydrating: true,
        ..MockBackendConfig::default()
    });

    let elem = view(vec![when(
        || true,
        || text("THEN-ARM").into(),
        || text("ELSE-ARM").into(),
    )]);
    let _owner = rt.render(elem.into());

    let events = rt.events();
    assert!(
        !events.iter().any(|e| matches!(e, Event::ClearChildren { .. })),
        "when must NOT clear_children under hydration — the SSR branch \
         is already attached to the anchor. events: {events:#?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::CreateText { content } if content == "THEN-ARM")),
        "when must still build the active branch under hydration; events: {events:#?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, Event::CreateText { content } if content == "ELSE-ARM")),
        "the inactive branch must NOT be built; events: {events:#?}"
    );
}

/// COMPLEMENT: with hydration off (a normal mount), `when(...)` DOES
/// call `clear_children` on the anchor (its build-and-mount path
/// expects an empty placeholder, and a re-fire later clears the prior
/// branch). Pins the non-hydration half of the gate so a regression
/// that silently skipped clear under all conditions would surface
/// here, not as a memory leak in re-fire later.
#[test]
fn when_without_hydration_calls_clear_children() {
    let rt = TestRuntime::new(); // hydrating: false (default)

    let elem = view(vec![when(
        || true,
        || text("THEN").into(),
        || text("ELSE").into(),
    )]);
    let _owner = rt.render(elem.into());

    let events = rt.events();
    assert!(
        events.iter().any(|e| matches!(e, Event::ClearChildren { .. })),
        "when's normal-mount path still uses clear+rebuild; events: {events:#?}"
    );
}

/// Sanity: a static `view` tree (no reactive anchor) under hydration
/// produces no `ClearChildren` either — proving the guard checks above
/// aren't trivially true. The contrast against
/// `when_without_hydration_calls_clear_children` shows the difference
/// is in the reactive-anchor build path, not in walker baseline.
#[test]
fn static_tree_under_hydration_has_no_clear_children() {
    let rt = TestRuntime::with_config(MockBackendConfig {
        hydrating: true,
        ..MockBackendConfig::default()
    });

    let elem: Element = view(vec![text("plain").into()]).into();
    let _owner = rt.render(elem);

    let events = rt.events();
    assert!(
        !events.iter().any(|e| matches!(e, Event::ClearChildren { .. })),
        "no reactive control flow → no clear_children at all; events: {events:#?}"
    );
}
