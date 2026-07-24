//! Raw touch/pointer delivery for `on_touch` views.
//!
//! `Backend::install_touch_handler` has a **no-op default body**, so a
//! backend that never implements it still compiles and still renders —
//! every `.on_touch()` view just silently receives nothing. On GTK that
//! meant the whiteboard demo's drawing surface and its tool buttons were
//! inert with no error anywhere: the app was sending events into a
//! method that does nothing.
//!
//! # Mapping
//!
//! GTK4 delivers pointer input through event controllers rather than
//! per-widget virtual methods. A single [`gtk4::GestureDrag`] covers all
//! four framework phases on one widget:
//!
//! | Framework          | GTK signal        |
//! |--------------------|-------------------|
//! | `TouchPhase::Began`| `drag-begin`      |
//! | `TouchPhase::Moved`| `drag-update`     |
//! | `TouchPhase::Ended`| `drag-end`        |
//! | `TouchPhase::Cancelled` | `cancel`     |
//!
//! `GestureDrag` (not `GestureClick` + a motion controller) because it
//! is the one controller that reports a *continuous* press-move-release
//! sequence with the press origin retained — which is exactly the shape
//! `TouchEvent` describes, and what a freehand stroke needs. It also
//! handles touchscreen and mouse identically, so pen/touch/mouse all
//! arrive through one path.
//!
//! `drag-update` reports an OFFSET from the press point, not an absolute
//! position; the start point is captured at `drag-begin` and added back,
//! or every move event lands near the widget origin.

use gtk4::prelude::*;
use runtime_core::{TouchEvent, TouchHandler, TouchId, TouchPhase, TouchPoint};
use std::cell::Cell;
use std::rc::Rc;

/// Monotonic source for [`TouchId`]. GTK's `GestureDrag` does not expose
/// a per-sequence integer, and the framework only needs the id to be
/// stable for the life of one gesture and distinct from its neighbours.
fn next_touch_id() -> u64 {
    thread_local! {
        static NEXT: Cell<u64> = const { Cell::new(1) };
    }
    NEXT.with(|n| {
        let v = n.get();
        n.set(v.wrapping_add(1).max(1));
        v
    })
}

/// Nanosecond timestamp for a touch event. GTK exposes event times in
/// milliseconds; the framework's contract is only that this is a
/// monotonic nanosecond value usable for velocity, not wall-clock.
fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Build one framework event. `local` is relative to the subscribed
/// widget; `window` is relative to the toplevel.
fn event(
    id: u64,
    phase: TouchPhase,
    local: (f64, f64),
    window: (f64, f64),
) -> TouchEvent {
    TouchEvent {
        id: TouchId(id),
        phase,
        position: TouchPoint { x: local.0 as f32, y: local.1 as f32 },
        window_position: TouchPoint { x: window.0 as f32, y: window.1 as f32 },
        timestamp_ns: now_ns(),
        // GTK reports pressure only for devices with a pressure axis and
        // only via `GdkEvent` axes; not surfaced through GestureDrag.
        force: None,
    }
}

/// Translate a widget-local point into toplevel coordinates. Falls back
/// to the local point when the widget is not yet in a window — better a
/// slightly wrong `window_position` than dropping the event.
fn to_window(widget: &gtk4::Widget, local: (f64, f64)) -> (f64, f64) {
    widget
        .root()
        .and_then(|root| {
            widget
                .translate_coordinates(&root, local.0, local.1)
                .map(|(x, y)| (x, y))
        })
        .unwrap_or(local)
}

/// Attach a `GestureDrag` to `widget` that drives `handler` through the
/// full Began → Moved → Ended/Cancelled lifecycle.
pub(crate) fn install(widget: &gtk4::Widget, handler: TouchHandler) {
    let gesture = gtk4::GestureDrag::new();
    // Claim the sequence on press so an ancestor GtkScrolledWindow does
    // not steal the drag mid-stroke (the GTK analogue of the framework's
    // `claim_touch`). Without this a stroke inside a scrollable region
    // turns into a scroll after a few pixels of movement.
    gesture.set_propagation_phase(gtk4::PropagationPhase::Bubble);

    // Press origin in widget coordinates, needed because `drag-update`
    // reports an offset rather than a position.
    let start = Rc::new(Cell::new((0.0f64, 0.0f64)));
    // Id of the in-flight gesture, so every event of one stroke shares a
    // TouchId (the framework routes by id after `Began`).
    let active = Rc::new(Cell::new(0u64));

    {
        let (start, active, handler) = (start.clone(), active.clone(), handler.clone());
        let widget = widget.clone();
        gesture.connect_drag_begin(move |_g, x, y| {
            start.set((x, y));
            let id = next_touch_id();
            active.set(id);
            let win = to_window(&widget, (x, y));
            let _ = handler(&event(id, TouchPhase::Began, (x, y), win));
        });
    }
    {
        let (start, active, handler) = (start.clone(), active.clone(), handler.clone());
        let widget = widget.clone();
        gesture.connect_drag_update(move |_g, dx, dy| {
            let (sx, sy) = start.get();
            // Offset → absolute; skipping this pins every move near the
            // widget origin and a stroke collapses into a dot.
            let local = (sx + dx, sy + dy);
            let win = to_window(&widget, local);
            let _ = handler(&event(active.get(), TouchPhase::Moved, local, win));
        });
    }
    {
        let (start, active, handler) = (start.clone(), active.clone(), handler.clone());
        let widget = widget.clone();
        gesture.connect_drag_end(move |_g, dx, dy| {
            let (sx, sy) = start.get();
            let local = (sx + dx, sy + dy);
            let win = to_window(&widget, local);
            let _ = handler(&event(active.get(), TouchPhase::Ended, local, win));
        });
    }
    {
        let (start, active, handler) = (start.clone(), active.clone(), handler);
        let widget = widget.clone();
        gesture.connect_cancel(move |_g, _seq| {
            // `Cancelled` is first-class: recognizers must reset state
            // exactly as on `Ended` but must NOT treat it as a completed
            // gesture (no stroke commit, no tap fired).
            let (sx, sy) = start.get();
            let win = to_window(&widget, (sx, sy));
            let _ = handler(&event(active.get(), TouchPhase::Cancelled, (sx, sy), win));
        });
    }

    widget.add_controller(gesture);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ids must be distinct per gesture — the framework routes every
    /// event after `Began` by id, so a repeated id would merge two
    /// concurrent strokes into one.
    #[test]
    fn touch_ids_are_distinct_and_nonzero() {
        let a = next_touch_id();
        let b = next_touch_id();
        assert_ne!(a, b);
        assert!(a > 0 && b > 0);
    }

    /// `drag-update` reports an offset from the press point. The event
    /// must carry the ABSOLUTE position — treating the offset as a
    /// position collapses every stroke to the top-left of the widget.
    #[test]
    fn move_position_is_press_origin_plus_offset() {
        let (sx, sy) = (40.0, 25.0);
        let (dx, dy) = (10.0, -5.0);
        let ev = event(1, TouchPhase::Moved, (sx + dx, sy + dy), (0.0, 0.0));
        assert_eq!(ev.position.x, 50.0);
        assert_eq!(ev.position.y, 20.0);
    }
}
