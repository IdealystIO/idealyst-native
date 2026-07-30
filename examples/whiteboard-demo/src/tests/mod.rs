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
// The kernel `World` behind the facade's test-support seam: unit tests need
// enter/flush control that no author-facing API exposes.
use runtime_core::__World as World;
use std::cell::RefCell;
use std::rc::Rc;

/// A fresh single-canvas board, matching `app()`'s seed. Shared by the document
/// and navigation tests.
///
/// Signal creation needs an entered world, and writes STAGE until that world
/// flushes — the app gets a flush per event from the backend's driver, so a test
/// that chains two document ops has to [`Board::settle`] between them or the
/// second op reads the first's pre-write values.
struct Board {
    world: World,
    store: CanvasStore,
    strokes: Strokes,
    active: Signal<usize>,
    version: Signal<u64>,
    ids: Signal<Vec<u64>>,
    next_id: Signal<u64>,
}

impl Board {
    /// Commit the staged writes — the test-side stand-in for the backend's
    /// post-event flush.
    fn settle(&self) {
        self.world.flush();
    }
}

fn board() -> Board {
    let store: CanvasStore = Rc::new(RefCell::new(vec![CanvasDoc::default()]));
    let strokes: Strokes = Rc::new(RefCell::new(Vec::new()));
    let world = World::new();
    let (active, version, ids, next_id) = world.enter(|| {
        (
            runtime_core::signal(0usize),  // active
            runtime_core::signal(0u64),    // version
            runtime_core::signal(vec![0]), // canvas_ids
            runtime_core::signal(1u64),    // next_id
        )
    });
    Board { world, store, strokes, active, version, ids, next_id }
}

/// A one-point stroke, for asserting stroke counts.
fn dot() -> Stroke {
    Stroke { points: vec![(1.0, 1.0)], width: 2.0, rgba: (0, 0, 0, 255), ink: false }
}
