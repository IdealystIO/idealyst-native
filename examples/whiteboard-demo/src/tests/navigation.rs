//! Canvas navigation: key/swipe → `CanvasAction`, and applying it to the store.

use super::{board, dot};
use crate::{add_canvas, apply_canvas_action, key_action, swipe_action, CanvasAction};

#[test]
fn key_action_maps_navigation_keys() {
    assert_eq!(key_action("ArrowLeft"), Some(CanvasAction::Prev));
    assert_eq!(key_action("ArrowRight"), Some(CanvasAction::Next));
    assert_eq!(key_action("+"), Some(CanvasAction::Add));
    assert_eq!(key_action("="), Some(CanvasAction::Add)); // unshifted +
    assert_eq!(key_action("-"), Some(CanvasAction::Remove));
    assert_eq!(key_action("_"), Some(CanvasAction::Remove));
    assert_eq!(key_action("a"), None);
    assert_eq!(key_action("Enter"), None);
}

#[test]
fn swipe_action_is_horizontal_and_thresholded() {
    let t = 48.0;
    // Left (−x) → Next, right (+x) → Prev, past threshold + horizontal-dominant.
    assert_eq!(swipe_action(-60.0, 5.0, t), Some(CanvasAction::Next));
    assert_eq!(swipe_action(60.0, 5.0, t), Some(CanvasAction::Prev));
    // Below threshold → nothing.
    assert_eq!(swipe_action(-20.0, 2.0, t), None);
    // Vertical-dominant (a scroll) → nothing even past the x threshold.
    assert_eq!(swipe_action(-60.0, 90.0, t), None);
}

// Prev/Next clamp at the ends; they move the active pointer + restore strokes.
#[test]
fn apply_prev_next_clamp_and_switch() {
    let (store, strokes, active, version, ids, next_id) = board();
    add_canvas(&store, &strokes, active, version, ids, next_id); // canvas 1 (active)
    // Prev → canvas 0; Prev again is a no-op (clamped).
    apply_canvas_action(CanvasAction::Prev, &store, &strokes, active, version, ids, next_id);
    assert_eq!(active.get(), 0);
    apply_canvas_action(CanvasAction::Prev, &store, &strokes, active, version, ids, next_id);
    assert_eq!(active.get(), 0, "clamped at the first canvas");
    // Next → canvas 1; Next again clamps at the last.
    apply_canvas_action(CanvasAction::Next, &store, &strokes, active, version, ids, next_id);
    assert_eq!(active.get(), 1);
    apply_canvas_action(CanvasAction::Next, &store, &strokes, active, version, ids, next_id);
    assert_eq!(active.get(), 1, "clamped at the last canvas");
}

// Remove deletes the current canvas, or clears it when it's the only one.
#[test]
fn apply_remove_deletes_or_clears_last() {
    let (store, strokes, active, version, ids, next_id) = board();
    add_canvas(&store, &strokes, active, version, ids, next_id); // 2 canvases
    apply_canvas_action(CanvasAction::Remove, &store, &strokes, active, version, ids, next_id);
    assert_eq!(ids.get().len(), 1, "deleted one canvas");
    // Now the last canvas: Remove clears it instead of deleting.
    strokes.borrow_mut().push(dot());
    apply_canvas_action(CanvasAction::Remove, &store, &strokes, active, version, ids, next_id);
    assert_eq!(ids.get().len(), 1, "last canvas survives");
    assert!(strokes.borrow().is_empty(), "but it was cleared");
}
