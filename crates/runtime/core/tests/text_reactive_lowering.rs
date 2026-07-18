//! `ui!`'s `text` lowering, 0.1.0 type-driven semantics: a **closure** text
//! content is reactive; a **value** is static. This replaces the 0.0.1 `.get()`
//! token scan that auto-wrapped bare `text { count.get() }` — reactivity is now
//! decided by the content's TYPE, and the reactive boundary is visible
//! (`text { move || … }`). A bare signal read in text is rejected at compile time
//! (a `compile_error!` footgun guard) rather than silently frozen; that path is
//! not exercised here (it doesn't compile by design).

#[path = "common/mod.rs"]
mod common;

use runtime_core::{signal, ui, Element, Signal};

use common::{Event, TestRuntime};

fn count_create_text(events: &[Event], needle: &str) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::CreateText { content } if content == needle))
        .count()
}

fn count_update_text(events: &[Event], needle: &str) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::UpdateText { content, .. } if content == needle))
        .count()
}

/// A **closure** text content is reactive — it re-evaluates and patches the text
/// in place (`UpdateText`) when a signal it reads changes.
#[test]
fn closure_text_content_is_reactive() {
    let rt = TestRuntime::new();
    let n: Signal<i32> = signal(1);

    let tree: Element = ui! {
        view {
            text { move || format!("n={}", n.get()) }
        }
    };
    let _owner = rt.render(tree);
    // A reactive text creates an empty node then patches content via its effect,
    // so "n=1" arrives as a create and/or an update — either proves it mounted.
    assert!(
        count_create_text(&rt.events(), "n=1") + count_update_text(&rt.events(), "n=1") >= 1,
        "initial reactive text present (events: {:?})",
        rt.events()
    );

    rt.backend_mut().clear_events();
    n.set(2);
    assert_eq!(
        count_update_text(&rt.events(), "n=2"),
        1,
        "closure text patches in place on signal change (events: {:?})",
        rt.events()
    );
}

/// A **value** text content is static — built once, never re-evaluated. Reading a
/// signal here (as a plain expression) would be a compile error by the footgun
/// guard, so a static text is a literal / non-signal value; changing an unrelated
/// signal produces no text update.
#[test]
fn value_text_content_is_static() {
    let rt = TestRuntime::new();
    let other: Signal<i32> = signal(0);

    let tree: Element = ui! {
        view {
            text { "static".to_string() }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_create_text(&rt.events(), "static"), 1, "static text built once");

    rt.backend_mut().clear_events();
    other.set(99);
    assert_eq!(
        count_update_text(&rt.events(), "static"),
        0,
        "a static text never patches on an unrelated signal (events: {:?})",
        rt.events()
    );
}
