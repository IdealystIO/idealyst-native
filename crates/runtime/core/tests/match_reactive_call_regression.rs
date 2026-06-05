//! Regression: `ui! { match key(state) { Enum::A => …, _ => … } }`
//! must be REACTIVE — re-render the active arm when a signal read by
//! `key` changes — exactly like the equivalent `if`/`else if` chain.
//!
//! The bug (field report "B4"): a `ui!` `match` whose scrutinee was a
//! bare function call reading signals (`match key(state)`) built once
//! and then NEVER re-rendered when the signals changed. Converting the
//! same logic to `if … else if … else` was reactive immediately.
//!
//! Root cause: `emit_match` had two reactive lowerings — a *structured*
//! `Element::Switch` (requires LITERAL arm keys) and a *closure*
//! `switch(..)` gated on a literal `.get()` substring in the scrutinee
//! (`condition_is_reactive`). A scrutinee like `key(state)` has no
//! `.get()` text (the read is inside `key`) and arms like
//! `Screen::Summary => …` are non-literal — so it fell through BOTH
//! reactive paths to the static plain-`match` arm. By contrast,
//! `if key(state) { … }` is always claimed by the structured
//! `try_emit_derived_call::<bool>` path, so it was reactive — hence the
//! asymmetry the reporter saw.
//!
//! Fix: `emit_match` now also treats the "reactive call" shape
//! (`fn(sig, …)` with bare-signal args) as reactive, rewriting
//! `key(state)` → `key(state.get())` inside the closure-`switch` so the
//! Effect subscribes — mirroring what `if`'s structured path does.
//!
//! Observation tool: the `MockBackend` event log (`CreateText`).

#[path = "common/mod.rs"]
mod common;

use runtime_core::{signal, ui, Element, Signal};

use common::{Event, TestRuntime};

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Summary,
    Detail,
    Other,
}

/// `key(state)` — the signal read lives INSIDE the function; the
/// argument is a bare `Signal`. This is exactly the field-report shape:
/// no literal `.get()` in the scrutinee, non-literal (enum) arm keys.
fn route(count: i32) -> Screen {
    if count == 0 {
        Screen::Summary
    } else if count == 1 {
        Screen::Detail
    } else {
        Screen::Other
    }
}

fn count_text(events: &[Event], needle: &str) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::CreateText { content } if content == needle))
        .count()
}

/// THE REGRESSION. Before the fix this lowered to a static plain
/// `match` and the second/third assertions failed (the arm never
/// changed — `summary` stayed mounted, `detail`/`other` never built).
#[test]
fn reactive_match_over_call_scrutinee_rerenders_on_signal_change() {
    let rt = TestRuntime::new();
    let state: Signal<i32> = signal!(0i32);

    let tree: Element = ui! {
        view {
            match route(state) {
                Screen::Summary => { text { "summary".to_string() } }
                Screen::Detail => { text { "detail".to_string() } }
                _ => { text { "other".to_string() } }
            }
        }
    };
    let _owner = rt.render(tree);

    // Initial: state=0 → Summary.
    assert_eq!(count_text(&rt.events(), "summary"), 1, "initial arm = Summary");
    assert_eq!(count_text(&rt.events(), "detail"), 0);
    assert_eq!(count_text(&rt.events(), "other"), 0);

    // state → 1 → Detail. This is the assertion that FAILED before the
    // fix: a static match never re-evaluated, so `detail` never built.
    rt.backend_mut().clear_events();
    state.set(1);
    assert_eq!(
        count_text(&rt.events(), "detail"),
        1,
        "arm must swap to Detail when the signal changes (events: {:?})",
        rt.events()
    );
    assert_eq!(count_text(&rt.events(), "summary"), 0);

    // state → 5 → Other (default arm).
    rt.backend_mut().clear_events();
    state.set(5);
    assert_eq!(count_text(&rt.events(), "other"), 1, "default arm for unmatched route");
}

/// Equivalence check: the SAME logic as an `if`/`else if`/`else` chain
/// over the same call shape is reactive too (it always was — this pins
/// the parity the reporter relied on, so a future change can't silently
/// re-break only the `match` side).
#[test]
fn reactive_if_chain_over_call_scrutinee_matches_match_behavior() {
    let rt = TestRuntime::new();
    let state: Signal<i32> = signal!(0i32);

    let tree: Element = ui! {
        view {
            if is_summary(state) {
                text { "summary".to_string() }
            } else if is_detail(state) {
                text { "detail".to_string() }
            } else {
                text { "other".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_text(&rt.events(), "summary"), 1, "initial branch");

    rt.backend_mut().clear_events();
    state.set(1);
    assert_eq!(count_text(&rt.events(), "detail"), 1, "else-if branch after change");

    // Nested `else if` lowers to nested `when`s, so the transition into
    // the final `else` can rebuild more than one anchor — assert the
    // branch became present (>= 1), not an exact count. The point of
    // this test is parity of *reactivity*, not anchor topology.
    rt.backend_mut().clear_events();
    state.set(5);
    assert!(count_text(&rt.events(), "other") >= 1, "else branch present after change");
    assert_eq!(count_text(&rt.events(), "detail"), 0, "detail torn down");
}

fn is_summary(count: i32) -> bool {
    count == 0
}

fn is_detail(count: i32) -> bool {
    count == 1
}

/// Guard arms still route through the reactive closure-`switch` for the
/// call shape (the structured path bails on guards). A guarded arm that
/// depends on the same signal re-evaluates on change.
#[test]
fn reactive_match_call_scrutinee_with_guarded_arm_rerenders() {
    let rt = TestRuntime::new();
    let state: Signal<i32> = signal!(0i32);

    let tree: Element = ui! {
        view {
            match route(state) {
                Screen::Summary => { text { "summary".to_string() } }
                s if matches!(s, Screen::Detail) => { text { "detail".to_string() } }
                _ => { text { "other".to_string() } }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_text(&rt.events(), "summary"), 1);

    rt.backend_mut().clear_events();
    state.set(1);
    assert_eq!(count_text(&rt.events(), "detail"), 1, "guarded arm reached after change");
}

// ---------------------------------------------------------------------------
// B5 (documented constraint + recommended workaround)
//
// A NESTED reactive `else if` chain whose DEEPEST arm MOVES a non-`Copy`
// capture fails to compile ("cannot move out of value, a captured
// variable in an `Fn` closure"): each `else if` lowers to a `when`
// nested inside the parent's `otherwise` `Fn` closure, which rebuilds
// the inner `when` (moving the capture inward) on every call. See the
// long comment at `emit_if` in `crates/runtime/macros/src/ui.rs`.
//
// This is the SAME `Fn`-branch constraint a flat reactive `switch`
// (a `match`) has — BUT a flat `match` is a single dispatcher closure
// that owns the value, so an in-arm `.clone()` reads it by ref and
// compiles. The B4 fix above makes the call-shape `match` reactive, so
// the flat-`match` workaround is now both reactive AND clone-friendly.
// This test pins that recommended path: a reactive call-shape `match`
// whose default arm clones a non-`Copy` value compiles, re-renders, and
// the cloned value reaches the rendered output.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppB5 {
    inner: std::rc::Rc<i32>,
}

fn render_app_b5(app: AppB5) -> Element {
    ui! { text { format!("app={}", app.inner) } }
}

/// `pick(a, b)` is the discriminant the `else if` chain would have used.
fn pick(a: bool, b: bool) -> u8 {
    if a {
        0
    } else if b {
        1
    } else {
        2
    }
}

#[test]
fn b5_flat_match_default_arm_clones_noncopy_capture_and_rerenders() {
    let rt = TestRuntime::new();
    let app = AppB5 { inner: std::rc::Rc::new(7) };
    let a: Signal<bool> = signal!(true);
    let b: Signal<bool> = signal!(false);

    // The deepest arm MOVES no capture — it CLONES `app` per invocation,
    // which compiles because the single `switch` dispatcher owns `app`.
    let tree: Element = ui! {
        view {
            match pick(a, b) {
                0 => { text { "a".to_string() } }
                1 => { text { "b".to_string() } }
                _ => { render_app_b5(app.clone()) }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_text(&rt.events(), "a"), 1, "initial arm (a)");

    // Flip into the default arm: it must build the cloned `app` content.
    rt.backend_mut().clear_events();
    a.set(false);
    assert_eq!(
        count_text(&rt.events(), "app=7"),
        1,
        "default arm renders the cloned non-Copy capture (events: {:?})",
        rt.events()
    );
}
