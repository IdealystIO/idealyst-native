//! `preserves_focus` plumbing — the focus-preserving press region every
//! combobox-shaped surface needs (`Backend::mark_preserves_focus`).
//!
//! Verifies the framework path the per-platform wiring relies on: the
//! `.preserves_focus(true)` builder stores the flag on
//! `Element::View` / `Element::Pressable`, and the walker calls
//! `Backend::mark_preserves_focus` for exactly the marked nodes.
//!
//! The native suppression mechanisms themselves (web capture-phase
//! `pointerdown` `preventDefault`, macOS `FlippedView` outside-click resign
//! exemption, iOS keyboard-dismiss tap exemption) are platform UI behavior
//! and aren't reachable from a host test — but they all key off exactly this
//! mark call, so proving it threads end-to-end pins the contract they share.

#[path = "common/mock_backend.rs"]
mod mock_backend;
#[path = "common/runtime.rs"]
mod runtime;

use mock_backend::{Event, NodeId};
use runtime::TestRuntime;
use runtime_core::{pressable, view, IntoElement};

/// REGRESSION (Autocomplete): the anchored option menu must reach the
/// backend as a focus-preserving region — without it, the menu's row press
/// blurs the input, the component's close-on-blur unmounts the row before
/// its click lands, and mouse selection is impossible.
#[test]
fn marked_view_reaches_backend() {
    let rt = TestRuntime::new();
    let _owner = rt.render(view(Vec::new()).preserves_focus(true).into_element());

    // The view is the root → its node is NodeId(0).
    rt.backend()
        .assert_any(|e| matches!(e, Event::MarkPreservesFocus { node } if *node == NodeId(0)));
}

/// Same for a pressable (Autocomplete's disclosure chevron): pressing it
/// must not blur the input, or close-on-blur flips `open` before the
/// chevron's own toggle runs and the menu reopens instead of closing.
#[test]
fn marked_pressable_reaches_backend() {
    let rt = TestRuntime::new();
    let _owner =
        rt.render(pressable(Vec::new(), || {}).preserves_focus(true).into_element());

    rt.backend()
        .assert_any(|e| matches!(e, Event::MarkPreservesFocus { node } if *node == NodeId(0)));
}

/// Unmarked nodes must NOT be marked — the flag defaults off and the walker
/// only calls the backend for opted-in nodes.
#[test]
fn unmarked_nodes_do_not_mark() {
    let rt = TestRuntime::new();
    let _owner = rt.render(
        view(vec![pressable(Vec::new(), || {}).into_element()]).into_element(),
    );

    assert!(
        !rt.backend()
            .events()
            .iter()
            .any(|e| matches!(e, Event::MarkPreservesFocus { .. })),
        "no node opted in, so mark_preserves_focus must not be called"
    );
}
