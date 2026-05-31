//! Regression: `ui!`'s `for` lowering emits FLAT siblings, never a
//! wrapper View per iteration.
//!
//! Bug: `emit_for`'s static (non-range, non-reactive) fallback ran the
//! loop body through `emit_block_as_primitive`, which collapses a
//! multi-node body into `view(vec![...])`. A loop whose body produced
//! several siblings therefore mounted one extra wrapper View per
//! iteration — corrupting flex layout, since a parent's `children` is
//! inherently a flat vector and the author wrote siblings, not a group.
//!
//! Observation lever: each wrapper View is a `CreateView` event. A
//! correct lowering of a 2-node body over a 2-element iterable emits
//! exactly ONE `CreateView` (the surrounding `View`) and four
//! `CreateText`s; the buggy lowering emitted three `CreateView`s.

use runtime_core::{ui, Element};

use crate::common::{Event, TestRuntime};

fn count_create_view(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::CreateView)).count()
}

fn count_create_text(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::CreateText { .. })).count()
}

/// Multi-node `for` body over a non-range iterable: the two `Text`
/// nodes per iteration must land as flat siblings of the parent
/// `View`, with no per-iteration wrapper.
#[test]
fn multi_node_for_body_emits_flat_siblings_no_wrapper_view() {
    let rt = TestRuntime::new();

    let tree: Element = ui! {
        view {
            for s in ["a", "b"] {
                text { s.to_string() }
                text { format!("{}!", s) }
            }
        }
    };
    let _owner = rt.render(tree);

    let events = rt.events();
    // Only the outer View — NOT one wrapper per iteration (which would
    // be 3). This is the assertion that fails before the fix.
    assert_eq!(
        count_create_view(&events),
        1,
        "expected exactly the outer View; a wrapper-per-iteration regressed the for-lowering: {events:?}"
    );
    // All four text leaves are direct children of the outer View.
    assert_eq!(count_create_text(&events), 4, "events: {events:?}");
}

/// Nested `for` in a multi-node body still flattens: the inner loop's
/// rows and the sibling `Text` all land directly under the outer View.
#[test]
fn nested_for_in_multi_node_body_flattens() {
    let rt = TestRuntime::new();

    let tree: Element = ui! {
        view {
            for group in [2usize, 1usize] {
                text { format!("header {}", group) }
                for i in 0..group {
                    text { format!("item {}", i) }
                }
            }
        }
    };
    let _owner = rt.render(tree);

    let events = rt.events();
    // No wrapper views anywhere — just the outer View.
    assert_eq!(
        count_create_view(&events),
        1,
        "nested for must flatten, not wrap: {events:?}"
    );
    // 2 headers + (2 + 1) items = 5 text leaves.
    assert_eq!(count_create_text(&events), 5, "events: {events:?}");
}

/// Single-node `for` body is unchanged: one sibling per iteration, no
/// wrapper. Guards against the fix accidentally introducing a wrapper
/// for the common case.
#[test]
fn single_node_for_body_unchanged() {
    let rt = TestRuntime::new();

    let tree: Element = ui! {
        view {
            for s in ["x", "y", "z"] {
                text { s.to_string() }
            }
        }
    };
    let _owner = rt.render(tree);

    let events = rt.events();
    assert_eq!(count_create_view(&events), 1, "events: {events:?}");
    assert_eq!(count_create_text(&events), 3, "events: {events:?}");
}
