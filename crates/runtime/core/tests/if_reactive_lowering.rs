//! `ui!`'s `if` lowering, 0.4.0 **inverted gate** — reactive is the safe default,
//! static is the proven-safe optimization.
//!
//! The rule `emit_if` implements (`condition_may_read_signal`):
//!   - A condition that **might read a signal** — ANY call / method-call /
//!     `.get()` *anywhere* in the tree (`if sig.get() > 0`, `if a.get() < b.get()`,
//!     `if items.len() > 0`, `if is_active(state)`), or an unrecognized shape →
//!     reactive `when(move || cond, …)`. The Effect subscribes to whatever the
//!     closure reads; a static one is an inert, ~free effect. Pre-0.4.0 this gate
//!     assumed static unless it spotted a *top-level* call or a literal `.get()`,
//!     so a call buried in a comparison/negation (`if items.len() > 0`,
//!     `if foo(x) < bar(y)`) silently froze.
//!   - A **provably signal-free** condition — a bare path/field (type-driven
//!     `StaticCond`/`ReactiveCond`), or a literal / `&&`/`||`/`!` / comparison of
//!     call-free operands (`if a == b`, `if x && y`, `if kind == Kind::Scope`) →
//!     plain static `if`, captures BORROWED (no `'static`/`Clone` ceremony).

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

/// 0.4.0 headline fix: a signal read BURIED IN A COMPARISON — not a top-level
/// call, no visible `.get()` at the `if` site — is now reactive. Pre-0.4.0 the
/// gate only caught top-level calls / literal `.get()`, so `if count() > 1`
/// lowered STATIC and silently froze. The inverted gate lowers any
/// call-containing condition reactively.
#[test]
fn comparison_with_buried_signal_read_is_reactive() {
    let rt = TestRuntime::new();
    let n: Signal<i32> = signal(0);
    // `count()` reads `n` internally; buried inside `count() > 1`, a `Binary`
    // that is neither a top-level call nor spells `.get()`.
    let count = move || n.get();

    let tree: Element = ui! {
        view {
            if count() > 1 {
                text { "high".to_string() }
            } else {
                text { "low".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_create_text(&rt.events(), "low"), 1, "initial branch = low");
    assert_eq!(count_create_text(&rt.events(), "high"), 0);

    // Flipping the signal read by the buried call must rebuild — pre-0.4.0 this
    // stayed "low" forever.
    rt.backend_mut().clear_events();
    n.set(5);
    assert_eq!(
        count_create_text(&rt.events(), "high"),
        1,
        "buried-call comparison rebuilds on the hidden signal (events: {:?})",
        rt.events()
    );
    assert!(count_clear_children(&rt.events()) >= 1, "old branch cleared before rebuild");
}

/// A `.get()` buried inside a compound comparison (`a.get() < b.get()`) — the
/// exact shape the design discussion centered on. Both signals are subscribed;
/// the branch flips when the comparison result flips.
#[test]
fn compound_get_comparison_is_reactive() {
    let rt = TestRuntime::new();
    let x: Signal<i32> = signal(1);
    let y: Signal<i32> = signal(5);

    let tree: Element = ui! {
        view {
            if x.get() < y.get() {
                text { "less".to_string() }
            } else {
                text { "gte".to_string() }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_create_text(&rt.events(), "less"), 1, "1 < 5 → less");

    // Change x so it no longer holds: 9 < 5 is false → rebuild to "gte".
    rt.backend_mut().clear_events();
    x.set(9);
    assert_eq!(
        count_create_text(&rt.events(), "gte"),
        1,
        "compound `.get()` comparison rebuilds when the result flips (events: {:?})",
        rt.events()
    );

    // A change to x that does NOT flip the comparison must NOT rebuild — proof
    // `when`'s value-dedup survives the inverted gate.
    rt.backend_mut().clear_events();
    x.set(20); // still 20 >= 5 → "gte" unchanged
    assert_eq!(
        count_create_text(&rt.events(), "gte"),
        0,
        "no rebuild when the boolean is unchanged (dedup preserved, events: {:?})",
        rt.events()
    );
}
