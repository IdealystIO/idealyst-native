//! `Element::Dynamic` — the generic reactive single-subtree, the
//! whole-element dual of a reactive `text(move || …)` leaf and the
//! foundation the 0.1.0 type-driven reactivity is built on.
//!
//! Three invariants, all observed through the `MockBackend` event log:
//!
//! 1. **Rebuilds on an eager read change.** A signal the build closure
//!    reads *while constructing the tree* is a rebuild dependency — a
//!    change tears down the old subtree (`ClearChildren`) and builds the
//!    new one (`CreateText` of the new content).
//! 2. **Does NOT rebuild on an unrelated signal.** A signal the closure
//!    never reads doesn't pin the Dynamic — changing it produces no
//!    rebuild.
//! 3. **Deferred reads update in place, no rebuild.** A reactive
//!    `text(move || inner.get())` built *inside* the Dynamic subscribes to
//!    `inner` via its OWN effect. Changing `inner` fires `UpdateText`, NOT
//!    a Dynamic rebuild — proving the walker's tracked-build /
//!    untracked-construct split (only the closure's eager reads count).

#[path = "common/mod.rs"]
mod common;

use runtime_core::{dynamic, signal, text, ui, Element, IntoElement, Signal};

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

fn count_clear_children(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::ClearChildren { .. })).count()
}

fn count_create_text_any(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::CreateText { .. })).count()
}

/// Invariant 1: an eager read in the build closure is a rebuild dep.
#[test]
fn dynamic_rebuilds_subtree_when_eager_read_changes() {
    let rt = TestRuntime::new();
    let flag: Signal<bool> = signal(true);

    // `flag.get()` is read EAGERLY while building the subtree, so it's a
    // rebuild dependency.
    let d = dynamic(move || {
        if flag.get() {
            ui! { text { "a".to_string() } }
        } else {
            ui! { text { "b".to_string() } }
        }
    });
    let tree: Element = ui! { view { d } };
    let _owner = rt.render(tree);

    // Initial: flag=true → "a".
    assert_eq!(count_create_text(&rt.events(), "a"), 1, "initial subtree = a");
    assert_eq!(count_create_text(&rt.events(), "b"), 0);

    // Flip: the Dynamic tears down the old subtree and builds the new one.
    rt.backend_mut().clear_events();
    flag.set(false);
    assert_eq!(
        count_create_text(&rt.events(), "b"),
        1,
        "subtree rebuilds to b on eager-read change (events: {:?})",
        rt.events()
    );
    assert!(
        count_clear_children(&rt.events()) >= 1,
        "old subtree is cleared before the rebuild"
    );
}

/// End-to-end: a bare **closure child** in `ui!` is reactive — the
/// type-driven boundary works through the macro's plain expression
/// passthrough (via the `ChildList for Fn` blanket impl), no `.get()` scan,
/// no macro special-casing. This is the 0.1.0 headline in one test.
#[test]
fn closure_child_in_ui_is_reactive() {
    let rt = TestRuntime::new();
    let n: Signal<i32> = signal(0);

    // `move || …` sits in child position as an ordinary Rust expression; the
    // `ChildList for Fn` impl splices it as a `dynamic` subtree.
    let tree: Element = ui! {
        view {
            move || if n.get() == 0 {
                ui! { text { "zero".to_string() } }
            } else {
                ui! { text { "nonzero".to_string() } }
            }
        }
    };
    let _owner = rt.render(tree);
    assert_eq!(count_create_text(&rt.events(), "zero"), 1, "initial closure child");

    rt.backend_mut().clear_events();
    n.set(1);
    assert_eq!(
        count_create_text(&rt.events(), "nonzero"),
        1,
        "closure child rebuilds on eager-read change (events: {:?})",
        rt.events()
    );
}

/// Invariant 2: a signal the closure never reads doesn't trigger a rebuild.
#[test]
fn dynamic_does_not_rebuild_on_unrelated_signal() {
    let rt = TestRuntime::new();
    let read: Signal<bool> = signal(true);
    let unrelated: Signal<i32> = signal(0);

    let d = dynamic(move || {
        // Only `read` is touched; `unrelated` is never read here.
        if read.get() {
            ui! { text { "x".to_string() } }
        } else {
            ui! { text { "y".to_string() } }
        }
    });
    let tree: Element = ui! { view { d } };
    let _owner = rt.render(tree);
    assert_eq!(count_create_text(&rt.events(), "x"), 1, "initial subtree");

    // Changing a signal the closure never read must NOT rebuild.
    rt.backend_mut().clear_events();
    unrelated.set(42);
    assert_eq!(
        count_create_text(&rt.events(), "x"),
        0,
        "no rebuild for an unread signal (events: {:?})",
        rt.events()
    );
    assert_eq!(count_clear_children(&rt.events()), 0, "no teardown either");
}

/// Invariant 3: a reactive leaf built INSIDE the Dynamic subscribes via its
/// own effect — an update to its dep patches in place (`UpdateText`) instead
/// of rebuilding the whole Dynamic subtree. This pins the tracked-build /
/// untracked-construct split.
#[test]
fn dynamic_defers_inner_reactive_reads_no_rebuild() {
    let rt = TestRuntime::new();
    let inner: Signal<i32> = signal(1);

    // `inner` is NOT read eagerly by the Dynamic closure — it's read inside
    // the reactive `text` closure, which owns its own effect.
    let d = dynamic(move || {
        text(move || format!("n={}", inner.get())).into_element()
    });
    let tree: Element = ui! { view { d } };
    let _owner = rt.render(tree);
    // A reactive text creates an empty node then patches content via its
    // effect, so the initial "n=1" may arrive as either a create or an
    // update — accept either, we only need the node present.
    assert!(
        count_create_text(&rt.events(), "n=1") + count_update_text(&rt.events(), "n=1") >= 1,
        "initial reactive text present (events: {:?})",
        rt.events()
    );

    // Change the inner dep. A patch-in-place (correct) produces ONLY an
    // `UpdateText`; a Dynamic rebuild (wrong) would `ClearChildren` the
    // anchor and build a fresh text node (`CreateText`). Assert the former,
    // negate the latter — this is the tracked-build/untracked-construct split.
    rt.backend_mut().clear_events();
    inner.set(2);
    assert_eq!(
        count_update_text(&rt.events(), "n=2"),
        1,
        "inner reactive text updates in place (events: {:?})",
        rt.events()
    );
    assert_eq!(
        count_create_text_any(&rt.events()),
        0,
        "no fresh text node → the Dynamic did NOT rebuild (the inner effect owns the dep)"
    );
    assert_eq!(
        count_clear_children(&rt.events()),
        0,
        "no subtree teardown for a deferred inner read"
    );
}
