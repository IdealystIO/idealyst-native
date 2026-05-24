//! Primitive dispatch — each `Primitive` variant produces the
//! expected sequence of backend calls when rendered.
//!
//! For each primitive: mount it through `render()`, assert the
//! MockBackend's event log contains the expected `Create*` /
//! `Insert` / etc. sequence. Catches "I added a primitive variant
//! and forgot a walker arm" regressions.

use runtime_core::{button, text, view, Primitive};

use crate::common::{Event, TestRuntime};

/// Mounting a single `text("hi")` produces one CreateText, one
/// Finish.
#[test]
fn text_mounts() {
    let rt = TestRuntime::new();
    let _owner = rt.render(text("hi").into());

    let events = rt.events();
    assert!(
        events.iter().any(|e| matches!(e, Event::CreateText { content } if content == "hi")),
        "expected CreateText with 'hi', got: {:#?}",
        events
    );
}

/// Mounting an empty `view(vec![])` produces one CreateView, one
/// Finish.
#[test]
fn empty_view_mounts() {
    let rt = TestRuntime::new();
    let _owner = rt.render(view(Vec::<Primitive>::new()).into());

    let events = rt.events();
    assert!(events.iter().any(|e| matches!(e, Event::CreateView)));
}

/// `view([text, text])` produces CreateView, CreateText, Insert,
/// CreateText, Insert.
#[test]
fn view_with_text_children_inserts_in_order() {
    let rt = TestRuntime::new();
    let _owner = rt.render(
        view(vec![text("one").into(), text("two").into()]).into(),
    );

    let events = rt.events();

    // Two CreateText events, in correct content order.
    let text_contents: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::CreateText { content } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text_contents, vec!["one", "two"]);

    // At least two Insert events.
    let insert_count = events
        .iter()
        .filter(|e| matches!(e, Event::Insert { .. }))
        .count();
    assert!(insert_count >= 2, "expected 2+ Insert events, got {insert_count}");
}

/// `button("click")` produces CreateButton with the label captured.
#[test]
fn button_mounts() {
    let rt = TestRuntime::new();
    let _owner = rt.render(button("click", || {}).into());

    rt.backend().assert_any(|e| {
        matches!(e, Event::CreateButton { label } if label == "click")
    });
}

/// Nested views: `view([view([text])])` should still produce the
/// correct create + insert tree.
#[test]
fn nested_view_tree() {
    let rt = TestRuntime::new();
    let _owner = rt.render(
        view(vec![view(vec![text("nested").into()]).into()]).into(),
    );

    let events = rt.events();
    // Two CreateView calls (outer + inner).
    let view_count = events
        .iter()
        .filter(|e| matches!(e, Event::CreateView))
        .count();
    assert_eq!(view_count, 2);

    // One CreateText.
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::CreateText { content } if content == "nested")));
}

/// Mounting then dropping the Owner triggers the backend's cleanup
/// path. We don't yet have per-primitive release events for plain
/// View/Text (no `release_view`/`release_text` exists), but
/// `on_node_unstyled` fires for styled nodes. Verify the framework
/// at least doesn't panic on drop.
#[test]
fn drop_owner_does_not_panic() {
    let rt = TestRuntime::new();
    let owner = rt.render(view(vec![text("bye").into()]).into());
    drop(owner);
    // If we got here, the drop path didn't panic.
}
