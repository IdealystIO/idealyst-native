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
