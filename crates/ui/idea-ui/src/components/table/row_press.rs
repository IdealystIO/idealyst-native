//! One press handler for a table row's surface (a virtual table's cell, a
//! `TableCards` row): a tap opens the row, and a secondary press or a hold
//! opens its menu.
//!
//! Both gestures live in ONE `on_touch` slot because there is no gesture
//! arena across views: whichever handler consumes `Began` owns the touch,
//! and the row's tap consumes it. A `long_press` around the table, or
//! beside the tap on an ancestor, never saw a `Began`.
//!
//! The hold wins over the tap: a tap's window (750 ms) outlasts the hold
//! (500 ms), so without cancelling it a 600 ms press would open both the
//! menu and the row. When the hold fires the tap is reset, its press
//! highlight clears, and the rest of that touch is the hold's.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::{
    pointer_button, LongPress, LongPressRecognizer, Recognizer, RecognizerCtx, Tap, TapRecognizer, TouchEvent,
    TouchHandler, TouchPhase, TouchPoint, TouchResponse,
};

/// A row's menu callback: the press's point in window coordinates, for
/// anchoring the menu where the reader pressed.
pub(crate) type RowMenu = Rc<dyn Fn(TouchPoint)>;

/// `None` when there is nothing to handle (no tap, no menu): the row then
/// installs no handler and presses bubble past it.
pub(crate) fn row_press(
    on_tap: Option<Rc<dyn Fn()>>,
    on_menu: Option<RowMenu>,
    on_pressed: impl Fn(bool) + 'static,
) -> Option<TouchHandler> {
    if on_tap.is_none() && on_menu.is_none() {
        return None;
    }
    let tap = on_tap.map(|f| Rc::new(RefCell::new(Tap::new(TapRecognizer::new(), move || f()))));
    let was_pressed = Rc::new(Cell::new(false));
    let on_pressed: Rc<dyn Fn(bool)> = Rc::new(on_pressed);
    // Where the current touch began, for a hold's menu.
    let began_at = Rc::new(Cell::new(TouchPoint { x: 0.0, y: 0.0 }));
    let hold = on_menu.clone().map(|menu| {
        let (tap, was_pressed, on_pressed, began_at) =
            (tap.clone(), was_pressed.clone(), on_pressed.clone(), began_at.clone());
        let rec = Rc::new(RefCell::new(LongPress::new(LongPressRecognizer::new(), move || {
            // The hold has it: the tap must not fire on release.
            if let Some(tap) = &tap {
                tap.borrow_mut().reset(true);
            }
            if was_pressed.replace(false) {
                on_pressed(false);
            }
            menu(began_at.get());
        })));
        // Fires straight from its timer, ungated (as `long_press` does).
        // Weak: the notifier lives in the recognizer it polls.
        let weak = Rc::downgrade(&rec);
        rec.borrow_mut().set_async_notifier(Rc::new(move || {
            if let Some(r) = weak.upgrade() {
                r.borrow_mut().poll_async(&RecognizerCtx::UNGATED);
            }
        }));
        rec
    });
    Some(Rc::new(move |ev: &TouchEvent| -> TouchResponse {
        if ev.phase == TouchPhase::Began {
            // A secondary press arrives as a lone `Began` (no `Ended`
            // follows), so it opens the menu and tracks nothing.
            if pointer_button().opens_context_menu() {
                return match &on_menu {
                    Some(menu) => {
                        menu(ev.window_position);
                        TouchResponse::CONSUMED
                    }
                    None => TouchResponse::IGNORED,
                };
            }
            began_at.set(ev.window_position);
        }
        let mut consumed = false;
        if let Some(hold) = &hold {
            consumed |= hold.borrow_mut().drive(ev, &RecognizerCtx::UNGATED).response.consumed;
        }
        if let Some(tap) = &tap {
            consumed |= tap.borrow_mut().drive(ev, &RecognizerCtx::UNGATED).response.consumed;
            // Read after the update and outside the borrow: the callback
            // may write a signal whose subscribers re-enter here.
            let pressed = tap.borrow().is_pressed();
            if pressed != was_pressed.get() {
                was_pressed.set(pressed);
                on_pressed(pressed);
            }
        }
        if consumed { TouchResponse::CONSUMED } else { TouchResponse::IGNORED }
    }))
}

/// A row's surface that also starts a drag (`Table(drag_from_row = true)`):
/// `press` (the row's tap and menu, if any) and `drag` hear the same touch
/// until the drag picks the row up; from then on the drag has it, and the
/// press is cancelled, so the release opens nothing and the press tint
/// clears.
///
/// With `gated`, the drag hears nothing until [`RowDragTouch::arm`]: on a
/// touch screen the hold opens the row's menu, and the menu's callback arms
/// the drag, so moving on from the hold picks the row up. The drag then
/// starts from where the finger is when it is armed.
pub(crate) struct RowDragTouch {
    pub(crate) handler: TouchHandler,
    /// Start feeding the drag (gated mode), from the touch's last point.
    pub(crate) arm: Rc<dyn Fn()>,
    /// Call as the drag picks the row up: the press stands down.
    pub(crate) committed: Rc<dyn Fn()>,
}

///
/// `held`, when given, says whether the drag has picked the row up; it is
/// asked after each event, for a drag with no commit hook of its own (a
/// plain table's row `Draggable`). Otherwise call `committed`.
pub(crate) fn row_drag_touch(
    press: Option<TouchHandler>,
    drag: Rc<RefCell<Option<TouchHandler>>>,
    gated: bool,
    held: Option<Rc<dyn Fn() -> bool>>,
) -> RowDragTouch {
    let dragging = Rc::new(Cell::new(false));
    let armed = Rc::new(Cell::new(!gated));
    let last: Rc<Cell<Option<TouchEvent>>> = Rc::new(Cell::new(None));
    let feed = {
        let drag = drag.clone();
        move |ev: &TouchEvent| -> TouchResponse {
            // Cloned out: the drag's callback may re-enter this table.
            let d = drag.borrow().clone();
            d.map_or(TouchResponse::IGNORED, |d| d(ev))
        }
    };
    let committed: Rc<dyn Fn()> = {
        let (dragging, last, press) = (dragging.clone(), last.clone(), press.clone());
        Rc::new(move || {
            if dragging.replace(true) {
                return;
            }
            if let (Some(press), Some(ev)) = (&press, last.get()) {
                press(&TouchEvent { phase: TouchPhase::Cancelled, ..ev });
            }
        })
    };
    let arm: Rc<dyn Fn()> = {
        let (armed, last, feed) = (armed.clone(), last.clone(), feed.clone());
        Rc::new(move || {
            if armed.replace(true) {
                return;
            }
            if let Some(ev) = last.get() {
                feed(&TouchEvent { phase: TouchPhase::Began, ..ev });
            }
        })
    };
    let committed_here = committed.clone();
    let handler: TouchHandler = Rc::new(move |ev: &TouchEvent| -> TouchResponse {
        if ev.phase == TouchPhase::Began {
            dragging.set(false);
            armed.set(!gated);
        }
        last.set(Some(*ev));
        let p = match (&press, dragging.get()) {
            (Some(press), false) => press(ev),
            _ => TouchResponse::IGNORED,
        };
        let d = if armed.get() { feed(ev) } else { TouchResponse::IGNORED };
        if held.as_ref().is_some_and(|held| held()) {
            committed_here();
        }
        TouchResponse { consumed: p.consumed || d.consumed, claim: p.claim || d.claim }
    });
    RowDragTouch { handler, arm, committed }
}
