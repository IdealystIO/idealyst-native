//! `ui!`'s `if` lowering, 0.1.0 semantics — the fix for the 0.0.1 silent-freeze
//! footgun plus the guard against over-reaching into gratuitous reactivity.
//!
//! The rule `emit_if` implements:
//!   - A `.get()` anywhere in the condition, OR a **top-level call** condition
//!     (`if use_focus()()`, `if is_active(state)`) → reactive `when(move || cond,
//!     …)`. A call can read a signal that isn't spelled `.get()` at the call
//!     site; in 0.0.1 those were treated as static and silently frozen.
//!   - A pure **structural** condition (`if a == b`, `if !v.is_empty()`,
//!     `if x && y`) reads no signal by construction → plain static `if`, captures
//!     BORROWED (no `'static`/`Clone` ceremony).

#[path = "common/mod.rs"]
mod common;

use std::rc::Rc;

use runtime_core::{signal, ui, Element, Signal};

use common::{Event, TestRuntime};

fn count_create_text(events: &[Event], needle: &str) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::CreateText { content } if content == needle))
        .count()
}

fn count_clear_children(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::ClearChildren { .. })).count()
}

/// The headline fix: a condition that is a **call** whose signal read is NOT
/// spelled `.get()` at the call site is now reactive. `is_on()` reads `flag`
/// internally — the classic `use_focus()`-shaped hook that 0.0.1 froze.
#[test]
fn call_condition_hiding_a_signal_read_is_reactive() {
    let rt = TestRuntime::new();
    let flag: Signal<bool> = signal(true);

    // The `.get()` lives INSIDE `is_on`, so it is invisible at the `if` site —
    // `condition_is_reactive` (which scans the if-site tokens) returns false.
    // The top-level-call rule is what makes this live.
    let is_on = move || flag.get();
    let tree: Element = ui! {
        view {
            if is_on() {
                text { "on".to_string() }
            } else {
                text { "off".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_create_text(&rt.events(), "on"), 1, "initial branch = on");
    assert_eq!(count_create_text(&rt.events(), "off"), 0);

    // Flipping the signal the hidden `.get()` read must rebuild the branch —
    // in 0.0.1 this stayed "on" forever (the frozen footgun).
    rt.backend_mut().clear_events();
    flag.set(false);
    assert_eq!(
        count_create_text(&rt.events(), "off"),
        1,
        "call-condition `if` rebuilds on the hidden signal (events: {:?})",
        rt.events()
    );
    assert!(count_clear_children(&rt.events()) >= 1, "old branch cleared before rebuild");
}

/// The guard against over-reaching: a purely structural comparison reads no
/// signal, so it stays a plain static `if` with BORROWED captures. This test
/// exercising it *compiles at all* is the assertion — the branch references a
/// non-`Clone`-shared `Rc` that is still owned after the `ui!`, which is only
/// possible because the branch borrows rather than moving into a `'static`
/// `when` closure. If `emit_if` regressed to lowering structural conditions to
/// `when(move || …)`, this file would fail to compile.
#[test]
fn structural_condition_stays_static_with_borrowed_capture() {
    let rt = TestRuntime::new();
    let shared: Rc<String> = Rc::new("hi".to_string());
    let kind: u8 = 2;

    let tree: Element = ui! {
        view {
            // `kind == 2` — no signal, no call → static `if`. The branch reads
            // `shared` by borrow; no `.clone()` of the `Rc`, no move.
            if kind == 2 {
                text { shared.as_str().to_string() }
            }
        }
    };
    let _owner = rt.render(tree);

    // `shared` is still owned here — proof the static branch borrowed it rather
    // than moving it into a reactive closure.
    assert_eq!(shared.as_str(), "hi");
    assert_eq!(count_create_text(&rt.events(), "hi"), 1, "static branch mounted once");
}
