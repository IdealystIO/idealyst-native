//! Unit tests for the drawing/document model ([`crate::document`]) and the
//! camera-clamp helpers, split by topic. They target `pub(crate)` items reached
//! via `crate::…`, so they live inside the crate (not the `tests/` integration
//! directory, which can only see the public API).

mod camera;
mod color;
mod documents;
mod navigation;

use crate::{CanvasDoc, CanvasStore, Stroke, Strokes};
use runtime_core::Signal;
use std::cell::RefCell;
use std::rc::Rc;

/// A fresh single-canvas board, matching `app()`'s seed. Shared by the document
/// and navigation tests. Returns `(store, strokes, active, version, ids, next_id)`.
fn board() -> (CanvasStore, Strokes, Signal<usize>, Signal<u64>, Signal<Vec<u64>>, Signal<u64>) {
    let store: CanvasStore = Rc::new(RefCell::new(vec![CanvasDoc::default()]));
    let strokes: Strokes = Rc::new(RefCell::new(Vec::new()));
    (
        store,
        strokes,
        Signal::new(0usize),  // active
        Signal::new(0u64),    // version
        Signal::new(vec![0]), // canvas_ids
        Signal::new(1u64),    // next_id
    )
}

/// A one-point stroke, for asserting stroke counts.
fn dot() -> Stroke {
    Stroke { points: vec![(1.0, 1.0)], width: 2.0, rgba: (0, 0, 0, 255), ink: false }
}
