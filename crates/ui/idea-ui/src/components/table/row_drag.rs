//! [`RowDrag`] — a virtualized [`Table`](super::Table)'s row drag offered
//! to a drag-and-drop context outside the table
//! (`Table(row_drag = Some(RowDrag::new(&ctx, payload)))`).
//!
//! The table's grip drag reorders rows; with a `RowDrag` the same drag is
//! also a `dnd` drag of the row's payload. Over one of the context's drop
//! targets the target takes it (`is_over`, then `on_drop`) and the table
//! does not reorder; anywhere inside the table it reorders as before.
//!
//! The payload type is erased here: `Table` is not generic, and the table
//! only ever needs to start the drag, report the pointer and end it.

use std::cell::RefCell;
use std::rc::Rc;

use dnd::{DragContext, DragPreview, DragSession};
use runtime_core::TouchPoint;

/// A drag-and-drop context a table's row drag joins, and what a row
/// carries into it.
///
/// ```ignore
/// let ctx: DragContext<Uuid> = DragContext::new();
/// let tasks = tasks_signal;
/// Table(
///     on_reorder = Some(reorder),
///     row_drag = Some(RowDrag::new(&ctx, move |row| tasks.with(|t| t[row].id))),
///     virtualized = Some(rows),
/// )
/// // …and once, last in the app's root, so the dragged row draws over
/// // everything: { dnd::drag_layer(&ctx) }
/// ```
#[derive(Clone)]
pub struct RowDrag {
    start: Rc<dyn Fn(usize, TouchPoint, DragPreview) -> ActiveRowDrag>,
}

impl RowDrag {
    /// Row drags join `ctx`, carrying `payload(row)` — `row` is the index
    /// [`VirtualRows`](super::VirtualRows) reads.
    pub fn new<T: Clone + 'static>(ctx: &DragContext<T>, payload: impl Fn(usize) -> T + 'static) -> Self {
        let ctx = ctx.clone();
        Self {
            start: Rc::new(move |row, at, preview| {
                let session = Rc::new(RefCell::new(Some(ctx.start_drag(payload(row), at, Some(preview)))));
                let (moving, ending) = (session.clone(), session);
                ActiveRowDrag {
                    moved: Box::new(move |at, preview_at| {
                        moving.borrow().as_ref().is_some_and(|s: &DragSession<T>| s.moved(at, Some(preview_at)))
                    }),
                    finish: Box::new(move |at| ending.borrow_mut().take().is_some_and(|s| s.finish(at))),
                }
            }),
        }
    }

    /// Open the drag of `row` with the pointer at window `at`.
    pub(super) fn start(&self, row: usize, at: TouchPoint, preview: DragPreview) -> ActiveRowDrag {
        (self.start)(row, at, preview)
    }
}

/// One row drag in flight in the outside context. Dropping it without
/// [`finish`](Self::finish) cancels the drag there.
pub(super) struct ActiveRowDrag {
    moved: Box<dyn Fn(TouchPoint, (f32, f32)) -> bool>,
    finish: Box<dyn Fn(TouchPoint) -> bool>,
}

impl ActiveRowDrag {
    /// The pointer is at window `at` and the preview's top-left at
    /// `preview_at`. Whether one of the context's targets is under it.
    pub(super) fn moved(&self, at: TouchPoint, preview_at: (f32, f32)) -> bool {
        (self.moved)(at, preview_at)
    }

    /// Release at `at`. Whether a target took the row.
    pub(super) fn finish(self, at: TouchPoint) -> bool {
        (self.finish)(at)
    }
}
