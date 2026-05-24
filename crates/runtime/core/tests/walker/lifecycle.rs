//! Mount / unmount lifecycle: when/switch flipping, scope-drop
//! cleanup, primitive release events.
//!
//! These tests verify that the framework correctly tears down the
//! old subtree when a reactive condition changes. The MockBackend's
//! event log carries no "release_view" event (the framework doesn't
//! have one — Views are just dropped) but we can observe release
//! events for primitives that DO have them: Portal, Virtualizer,
//! Graphics, Navigator, External.

use runtime_core::{signal, text, view, when};

use crate::common::{Event, TestRuntime};

/// `when(true)` mounts the then-branch.
#[test]
fn when_true_mounts_then_branch() {
    let s = signal!(true);

    let rt = TestRuntime::new();
    let _owner = rt.render(
        when(move || s.get(), || text("then").into(), || text("else").into()),
    );

    let events = rt.events();
    assert!(
        events.iter().any(|e| matches!(e, Event::CreateText { content } if content == "then")),
        "then branch should mount when cond=true; events: {:#?}",
        events
    );
    assert!(
        !events.iter().any(|e| matches!(e, Event::CreateText { content } if content == "else")),
        "else branch should NOT mount when cond=true"
    );
}

/// `when(false)` mounts the else-branch.
#[test]
fn when_false_mounts_else_branch() {
    let s = signal!(false);
    let rt = TestRuntime::new();
    let _owner = rt.render(
        when(move || s.get(), || text("then").into(), || text("else").into()),
    );

    let events = rt.events();
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::CreateText { content } if content == "else")));
    assert!(!events
        .iter()
        .any(|e| matches!(e, Event::CreateText { content } if content == "then")));
}

/// Flipping the condition mounts the other branch.
#[test]
fn when_flip_mounts_other_branch() {
    let s = signal!(true);
    let rt = TestRuntime::new();
    let _owner = rt.render(
        when(move || s.get(), || text("then").into(), || text("else").into()),
    );

    // Initial state: then branch mounted.
    let initial_events = rt.events();
    let initial_then_count = initial_events
        .iter()
        .filter(|e| matches!(e, Event::CreateText { content } if content == "then"))
        .count();
    let initial_else_count = initial_events
        .iter()
        .filter(|e| matches!(e, Event::CreateText { content } if content == "else"))
        .count();
    assert_eq!(initial_then_count, 1);
    assert_eq!(initial_else_count, 0);

    // Flip.
    s.set(false);

    let after_events = rt.events();
    let after_else_count = after_events
        .iter()
        .filter(|e| matches!(e, Event::CreateText { content } if content == "else"))
        .count();
    assert_eq!(after_else_count, 1, "else branch mounted after flip");
}

/// Owner drop unmounts the entire tree without panic.
#[test]
fn owner_drop_unmounts_cleanly() {
    let rt = TestRuntime::new();
    {
        let _owner = rt.render(
            view(vec![
                text("a").into(),
                view(vec![text("b").into()]).into(),
                text("c").into(),
            ])
            .into(),
        );
        // Owner dropped here.
    }
    // No panic.
}
