//! Press, drag, release — picking a menu row with one continuous press.
//!
//! A button with `PressActivation::Down` opens its menu while the pointer
//! is still held. The press belongs to that button until it is released
//! (every backend delivers the rest of a press to the control that took the
//! down), so a row the pointer is dragged onto never sees a press of its
//! own and would never fire. This module is the other half: a row follows
//! the pointer through the app-level mouse listener (`on_pointer`), lights
//! up while it is under it, and fires when the primary button is released
//! over it.
//!
//! A finger works the same way through a second channel: the app-level
//! listener carries no touches, so the press-down control that holds the
//! finger reports where it goes (`on_held_press`) and the row follows that.
//!
//! # What counts as a drag-release
//!
//! A row fires on a primary `Up` when all of these hold ([`RowPress`]):
//!
//! - the press did NOT start on this row. One that did is an ordinary
//!   click and the row's own pressable handles it; firing here too would
//!   select twice.
//! - a `Move` put the pointer over the row first. A release with no motion
//!   onto the row is not a pick: it keeps a plain click on the trigger from
//!   selecting a row that happened to open under the stationary pointer.
//! - the release itself is over the row.
//!
//! Nothing here needs to know whether a button was held when the row
//! mounted, which the app-level source cannot tell (it starts tracking
//! buttons when its first listener registers, after the opening press).
//! A menu opened by a click or the keyboard just never produces an `Up`
//! without a `Down` of its own, so nothing fires; its rows light up under
//! the pointer exactly as their `hovered` state already does (the `active`
//! variant and the `hovered` state are the same wash).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_core::{
    on_held_press, on_pointer, on_scope_drop, pointer_root_origin, AppPointerEvent, HeldPressEvent,
    HeldPressPhase, MouseButton, PointerOutcome, PointerPhase, Signal, ViewportRect,
};

/// What one pointer event means for a row.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RowStep {
    /// The row's lit state after this event.
    pub lit: bool,
    /// The primary button was released over the row at the end of a press
    /// that started elsewhere: fire the row.
    pub pick: bool,
}

/// One row's view of the pointer stream. Pure: the caller says whether
/// each event is over the row.
#[derive(Default)]
pub(crate) struct RowPress {
    /// The press in flight started on this row (an ordinary click).
    own_press: Cell<bool>,
    /// A `Move` has put the pointer over the row.
    lit: Cell<bool>,
}

impl RowPress {
    pub(crate) fn on_event(&self, phase: PointerPhase, button: Option<MouseButton>, over: bool) -> RowStep {
        let primary = button == Some(MouseButton::Primary);
        match phase {
            PointerPhase::Down if primary => {
                self.own_press.set(over);
                // A press that starts here hands the row to its own
                // pressable (and its `pressed` / `hovered` states).
                if over {
                    self.lit.set(false);
                }
                RowStep { lit: self.lit.get(), pick: false }
            }
            PointerPhase::Move => {
                if !self.own_press.get() {
                    self.lit.set(over);
                }
                RowStep { lit: self.lit.get(), pick: false }
            }
            PointerPhase::Up if primary => {
                let pick = self.lit.get() && over && !self.own_press.get();
                self.own_press.set(false);
                // The pick closes the menu; a release elsewhere leaves the
                // row to the platform's own hover from here on.
                self.lit.set(false);
                RowStep { lit: false, pick }
            }
            _ => RowStep { lit: self.lit.get(), pick: false },
        }
    }
}

fn contains(r: ViewportRect, x: f32, y: f32) -> bool {
    r.width > 0.0 && r.height > 0.0 && x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

type RectFn = Rc<dyn Fn() -> Option<ViewportRect>>;

runtime_tls::thread_local! {
    /// The on-screen area of every mounted menu panel, in mount order
    /// (bottom to top). A row's own rect is not enough to say the pointer
    /// is over it — see [`row_under_pointer`].
    static PANELS: RefCell<Vec<(u64, RectFn)>> = const { RefCell::new(Vec::new()) };
    static NEXT_PANEL: Cell<u64> = const { Cell::new(0) };
}

/// Removes its panel from [`PANELS`] when dropped.
struct PanelEntry(u64);

impl Drop for PanelEntry {
    fn drop(&mut self) {
        let id = self.0;
        let _ = PANELS.try_with(|p| {
            if let Ok(mut p) = p.try_borrow_mut() {
                p.retain(|(pid, _)| *pid != id);
            }
        });
    }
}

/// Register a menu panel's on-screen rect for as long as the calling scope
/// lives. Call it while building the panel.
pub(crate) fn register_panel(rect: impl Fn() -> Option<ViewportRect> + 'static) {
    let id = NEXT_PANEL.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    PANELS.with(|p| p.borrow_mut().push((id, Rc::new(rect))));
    // The entry leaves with the scope. Dropping the closure unrun (no
    // world is entered, so nothing owns a scope) removes it at once, so a
    // panel built outside a mount cannot stay registered forever.
    let entry = PanelEntry(id);
    on_scope_drop(move || drop(entry));
}

/// How far a row may poke past its panel's sides and still be that
/// panel's row (sub-pixel layout rounding).
const ROW_IN_PANEL_SLACK: f32 = 1.0;

/// Whether the row at `row` is what the pointer at `(x, y)` is over: the
/// point is inside the row, and the row belongs to the TOPMOST panel under
/// the point.
///
/// Panels stack in mount order, so the last registered one containing the
/// point is the one on top. A flyout that flips on a narrow screen overlaps
/// its parent menu; under the overlap the parent's rows are covered, and
/// without this a release there picked the flyout row AND the parent row
/// hidden beneath it. A row has no handle on its panel, so "belongs to" is
/// geometric: a panel's rows span its width, which a different panel's
/// rows (offset to the side) do not. A row scrolled out of a capped panel
/// lies above or below it, where no panel is under the point at all.
fn row_under_pointer(row: ViewportRect, x: f32, y: f32) -> bool {
    if !contains(row, x, y) {
        return false;
    }
    // Snapshot first: a rect read can run backend code.
    let panels: Vec<RectFn> = PANELS.with(|p| p.borrow().iter().map(|(_, f)| f.clone()).collect());
    let topmost = panels.iter().rev().find_map(|rect| rect().filter(|r| contains(*r, x, y)));
    topmost.is_some_and(|p| {
        row.x >= p.x - ROW_IN_PANEL_SLACK && row.x + row.width <= p.x + p.width + ROW_IN_PANEL_SLACK
    })
}

/// Make the row measured by `rect` pickable by press-drag-release for as
/// long as the calling scope lives: `lit` follows the pointer while it is
/// over the row, and `pick` fires when the primary button is released
/// there at the end of a press that started elsewhere. Call it from the
/// row's component body (the listener is scoped like `on_pointer`).
pub(crate) fn follow_press(
    rect: impl Fn() -> Option<ViewportRect> + 'static,
    lit: Signal<bool>,
    pick: Rc<dyn Fn()>,
) {
    // One state for both channels. A press that arrives on both (an iPad
    // mouse is a touch contact and a pointer) picks once: the first `Up`
    // clears `lit`, so the second finds nothing to pick.
    let state = Rc::new(RowPress::default());
    let over = Rc::new(move |x: f32, y: f32| rect().is_some_and(|r| row_under_pointer(r, x, y)));
    let apply = {
        let state = state.clone();
        Rc::new(move |phase: PointerPhase, button: Option<MouseButton>, over: bool| {
            let step = state.on_event(phase, button, over);
            lit.set(step.lit);
            if step.pick {
                (pick)();
            }
        })
    };

    // Mouse, trackpad, pen: the app-level pointer source.
    {
        let (over, apply) = (over.clone(), apply.clone());
        on_pointer(move |ev: &AppPointerEvent| {
            // Wheel and non-primary buttons never change a row; skip the
            // measure for them.
            let relevant = match ev.phase {
                PointerPhase::Move => true,
                PointerPhase::Down | PointerPhase::Up => ev.button == Some(MouseButton::Primary),
                PointerPhase::Wheel => false,
            };
            if relevant && !ev.locked {
                // Event positions are app-root relative, rects viewport relative.
                let (ox, oy) = pointer_root_origin();
                apply(ev.phase, ev.button, over(ev.x + ox, ev.y + oy));
            }
            PointerOutcome::Default
        });
    }

    // A finger: the press-down control that holds the press reports where
    // it goes (`runtime_shared::held_press`), already in viewport pixels.
    on_held_press(move |ev: &HeldPressEvent| match ev.phase {
        HeldPressPhase::Moved => apply(PointerPhase::Move, None, over(ev.x, ev.y)),
        HeldPressPhase::Released => apply(PointerPhase::Up, Some(MouseButton::Primary), over(ev.x, ev.y)),
        // The system took the touch: nothing is picked, nothing stays lit.
        HeldPressPhase::Cancelled => apply(PointerPhase::Move, None, false),
    });
}

/// Keep `over` equal to "the pointer is inside `rect`" for as long as the
/// calling scope lives. The hover a view cannot get while a press is held
/// elsewhere: a SubMenu's flyout uses it to stay open while a held press
/// is dragged into it.
pub(crate) fn follow_pointer_over(rect: impl Fn() -> Option<ViewportRect> + 'static, over: Signal<bool>) {
    let rect = Rc::new(rect);
    {
        let rect = rect.clone();
        on_pointer(move |ev: &AppPointerEvent| {
            if ev.phase == PointerPhase::Move && !ev.locked {
                let (ox, oy) = pointer_root_origin();
                over.set(rect().is_some_and(|r| contains(r, ev.x + ox, ev.y + oy)));
            }
            PointerOutcome::Default
        });
    }
    on_held_press(move |ev: &HeldPressEvent| {
        if ev.phase == HeldPressPhase::Moved {
            over.set(rect().is_some_and(|r| contains(r, ev.x, ev.y)));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use MouseButton::{Primary, Secondary};
    use PointerPhase::{Down, Move, Up};

    fn step(lit: bool, pick: bool) -> RowStep {
        RowStep { lit, pick }
    }

    /// The gesture itself: the press started on the trigger (no `Down` the
    /// row ever saw, or one that was not over it), the pointer was dragged
    /// onto the row, and the button was released there.
    #[test]
    fn drag_onto_a_row_and_release_picks_it() {
        let row = RowPress::default();
        assert_eq!(row.on_event(Move, None, false), step(false, false));
        assert_eq!(row.on_event(Move, None, true), step(true, false), "lights up under the pointer");
        assert_eq!(row.on_event(Up, Some(Primary), true), step(false, true), "released over it: picked");
        // The press is over; a later stray release does nothing.
        assert_eq!(row.on_event(Up, Some(Primary), true), step(false, false));
    }

    /// Web reports the opening press's own `Down` after the menu has
    /// mounted (the compatibility `mousedown` follows the `pointerdown`
    /// that opened it). It lands on the trigger, not on a row, and must
    /// not stop the drag from picking.
    #[test]
    fn a_down_elsewhere_does_not_cancel_the_drag() {
        let row = RowPress::default();
        assert_eq!(row.on_event(Down, Some(Primary), false), step(false, false));
        row.on_event(Move, None, true);
        assert_eq!(row.on_event(Up, Some(Primary), true), step(false, true));
    }

    /// An ordinary click on a row is the row's own pressable's job. Firing
    /// here as well would select twice.
    #[test]
    fn regression_a_press_that_starts_on_the_row_is_not_a_drag_pick() {
        let row = RowPress::default();
        row.on_event(Move, None, true);
        assert_eq!(row.on_event(Down, Some(Primary), true), step(false, false));
        assert_eq!(row.on_event(Move, None, true), step(false, false), "its own press: not lit by the drag");
        assert_eq!(row.on_event(Up, Some(Primary), true), step(false, false), "no pick: the click fires it");
        // The next press, started elsewhere, drags onto it normally.
        row.on_event(Move, None, true);
        assert_eq!(row.on_event(Up, Some(Primary), true), step(false, true));
    }

    /// A click on the trigger with no motion must not pick a row that
    /// happened to open under the stationary pointer.
    #[test]
    fn regression_a_release_with_no_move_onto_the_row_picks_nothing() {
        let row = RowPress::default();
        assert_eq!(row.on_event(Up, Some(Primary), true), step(false, false));
    }

    #[test]
    fn release_off_the_row_picks_nothing_and_unlights_it() {
        let row = RowPress::default();
        row.on_event(Move, None, true);
        assert_eq!(row.on_event(Move, None, false), step(false, false), "dragged off again");
        row.on_event(Move, None, true);
        assert_eq!(row.on_event(Up, Some(Primary), false), step(false, false));
    }

    #[test]
    fn secondary_button_neither_picks_nor_disturbs_the_drag() {
        let row = RowPress::default();
        row.on_event(Move, None, true);
        assert_eq!(row.on_event(Down, Some(Secondary), true), step(true, false));
        assert_eq!(row.on_event(Up, Some(Secondary), true), step(true, false));
        assert_eq!(row.on_event(Up, Some(Primary), true), step(false, true));
    }

    #[test]
    fn contains_is_half_open_and_rejects_an_unmounted_rect() {
        let r = ViewportRect { x: 10.0, y: 20.0, width: 100.0, height: 30.0 };
        assert!(contains(r, 10.0, 20.0));
        assert!(contains(r, 109.9, 49.9));
        assert!(!contains(r, 110.0, 30.0), "the right edge belongs to the next thing");
        assert!(!contains(r, 50.0, 50.0), "the bottom edge belongs to the row below");
        assert!(!contains(ViewportRect::default(), 0.0, 0.0), "a zero rect is an unmounted handle");
    }
}
