//! Multi-canvas document ops: add / switch / delete / reset and `any_drawings`.

use super::{board, dot};
use crate::{add_canvas, any_drawings, delete_canvas, reset_canvases, switch_canvas};

// Adding a canvas snapshots the current drawing, appends an empty canvas, and
// switches to it (live strokes cleared, ids + active grow in lock-step).
#[test]
fn add_canvas_snapshots_and_switches() {
    let b = board();
    b.strokes.borrow_mut().push(dot()); // draw on canvas 0
    add_canvas(&b.store, &b.strokes, b.active, b.version, b.ids, b.next_id);
    b.settle();
    assert_eq!(b.active.get(), 1, "switched to the new canvas");
    assert_eq!(b.ids.get(), vec![0, 1], "ids grew in order");
    assert!(b.strokes.borrow().is_empty(), "new canvas is blank");
    assert_eq!(b.store.borrow()[0].strokes.len(), 1, "canvas 0's drawing was saved");
    assert_eq!(b.next_id.get(), 2, "id source advanced");
}

// Switching saves the current canvas and restores the target's strokes.
#[test]
fn switch_canvas_round_trips_strokes() {
    let b = board();
    b.strokes.borrow_mut().push(dot()); // canvas 0 has 1 stroke
    add_canvas(&b.store, &b.strokes, b.active, b.version, b.ids, b.next_id); // -> canvas 1
    b.settle();
    b.strokes.borrow_mut().push(dot());
    b.strokes.borrow_mut().push(dot()); // canvas 1 has 2 strokes
    switch_canvas(&b.store, &b.strokes, b.active, b.version, 0);
    b.settle();
    assert_eq!(b.active.get(), 0);
    assert_eq!(b.strokes.borrow().len(), 1, "canvas 0 restored");
    switch_canvas(&b.store, &b.strokes, b.active, b.version, 1);
    b.settle();
    assert_eq!(b.strokes.borrow().len(), 2, "canvas 1 restored");
}

// Deleting the active canvas removes it, re-clamps active to a live doc, and
// loads that doc's strokes; the last canvas can't be deleted.
#[test]
fn delete_canvas_reclamps_and_guards_last() {
    let b = board();
    add_canvas(&b.store, &b.strokes, b.active, b.version, b.ids, b.next_id); // canvas 1
    b.settle();
    b.strokes.borrow_mut().push(dot()); // canvas 1 has a stroke
    // Delete the active (idx 1) → falls back to canvas 0 (blank).
    delete_canvas(&b.store, &b.strokes, b.active, b.version, b.ids, 1);
    b.settle();
    assert_eq!(b.ids.get(), vec![0]);
    assert_eq!(b.active.get(), 0);
    assert!(b.strokes.borrow().is_empty(), "loaded canvas 0 (blank)");
    // Can't delete the only remaining canvas.
    delete_canvas(&b.store, &b.strokes, b.active, b.version, b.ids, 0);
    b.settle();
    assert_eq!(b.ids.get(), vec![0], "last canvas is protected");
}

// Deleting a canvas BEFORE the active one shifts the active index down so it
// still addresses the same document.
#[test]
fn delete_before_active_shifts_index() {
    let b = board();
    add_canvas(&b.store, &b.strokes, b.active, b.version, b.ids, b.next_id); // 1
    b.settle();
    add_canvas(&b.store, &b.strokes, b.active, b.version, b.ids, b.next_id); // 2 (active)
    b.settle();
    assert_eq!(b.active.get(), 2);
    delete_canvas(&b.store, &b.strokes, b.active, b.version, b.ids, 0); // remove first
    b.settle();
    assert_eq!(b.ids.get(), vec![1, 2]);
    assert_eq!(b.active.get(), 1, "active shifted down to still point at the same doc");
}

// Resetting collapses the board to one blank canvas regardless of prior state.
#[test]
fn reset_canvases_collapses_to_one_blank() {
    let b = board();
    b.strokes.borrow_mut().push(dot());
    add_canvas(&b.store, &b.strokes, b.active, b.version, b.ids, b.next_id);
    b.settle();
    add_canvas(&b.store, &b.strokes, b.active, b.version, b.ids, b.next_id);
    b.settle();
    reset_canvases(&b.store, &b.strokes, b.active, b.version, b.ids, b.next_id);
    b.settle();
    assert_eq!(b.store.borrow().len(), 1);
    assert_eq!(b.ids.get().len(), 1);
    assert_eq!(b.active.get(), 0);
    assert!(b.strokes.borrow().is_empty());
    assert!(!any_drawings(&b.store, &b.strokes, b.active), "a fresh board has no drawings");
}

// `any_drawings` sees the live active strokes AND strokes saved in other docs.
#[test]
fn any_drawings_spans_all_canvases() {
    let b = board();
    assert!(!any_drawings(&b.store, &b.strokes, b.active), "empty board");
    // Draw on canvas 0, move to a blank canvas 1 → still counts (canvas 0 saved).
    b.strokes.borrow_mut().push(dot());
    add_canvas(&b.store, &b.strokes, b.active, b.version, b.ids, b.next_id);
    b.settle();
    assert!(b.strokes.borrow().is_empty(), "on the blank canvas 1");
    assert!(any_drawings(&b.store, &b.strokes, b.active), "canvas 0 still has a drawing");
}
