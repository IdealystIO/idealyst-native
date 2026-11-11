//! The travel of a finger press that a press-down pressable took.
//!
//! A `pressable` with [`PressActivation::Down`](crate::PressActivation)
//! fires at the down and then owns the rest of that press: every backend
//! delivers the moves and the release to the control that took the down,
//! and to nobody else. Something the press opened — a menu — cannot see
//! where the finger goes next.
//!
//! This is where the control says so. While a FINGER (or stylus) press it
//! fired for is still down, the backend reports each move and the end of
//! the press here, and listeners follow it: a menu row lights up under the
//! finger and is picked when the finger lifts over it.
//!
//! ## Scope
//!
//! Touch contacts only. A mouse, trackpad or pen press is already visible
//! app-wide through [`pointer_input`](crate::pointer_input) (its `Move` and
//! `Up` arrive whatever control took the down), and that module's contract
//! excludes finger touches — this is their counterpart for the one case
//! that needs it. A backend whose touch contacts also reach
//! `pointer_input` may report both; listeners must tolerate a press
//! arriving on both channels.
//!
//! ## Coordinates
//!
//! Viewport (window) logical pixels — the space
//! [`TouchEvent::window_position`](crate::TouchEvent) and a handle's
//! `rect()` / `absolute_frame()` are in. No conversion is needed to test a
//! reported point against a handle rect.
//!
//! ## Backend contract
//!
//! From the press-down pressable's own touch stream, for a press that
//! fired its action at the down: [`report`] `Moved` for each move,
//! `Released` when the contact lifts, `Cancelled` when the system takes
//! the touch away. Nothing is reported for the down itself (the action
//! already says it happened), for a press that did not fire (a secondary
//! button), or for a release-mode pressable.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What a held press just did.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum HeldPressPhase {
    /// The contact moved.
    Moved,
    /// The contact lifted: the press is over, at this point.
    Released,
    /// The system took the touch away (an incoming call, a gesture
    /// takeover): the press is over and picked nothing.
    Cancelled,
}

/// One step of a held press, in viewport logical pixels.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HeldPressEvent {
    pub phase: HeldPressPhase,
    pub x: f32,
    pub y: f32,
}

type Handler = Rc<dyn Fn(&HeldPressEvent)>;

struct Entry {
    id: u64,
    alive: Rc<Cell<bool>>,
    handler: Handler,
}

runtime_tls::thread_local! {
    static LISTENERS: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

/// RAII registration from [`add_listener`]; dropping it removes the
/// listener.
#[must_use = "dropping a HeldPressListener removes it immediately"]
pub struct HeldPressListener {
    id: u64,
    alive: Rc<Cell<bool>>,
}

impl Drop for HeldPressListener {
    fn drop(&mut self) {
        self.alive.set(false);
        let id = self.id;
        let _ = LISTENERS.try_with(|l| {
            if let Ok(mut l) = l.try_borrow_mut() {
                l.retain(|e| e.id != id);
            }
        });
    }
}

/// Follow held presses until the guard drops.
pub fn add_listener(handler: impl Fn(&HeldPressEvent) + 'static) -> HeldPressListener {
    let alive = Rc::new(Cell::new(true));
    let id = NEXT_ID.with(|n| {
        n.set(n.get() + 1);
        n.get()
    });
    LISTENERS.with(|l| l.borrow_mut().push(Entry { id, alive: alive.clone(), handler: Rc::new(handler) }));
    HeldPressListener { id, alive }
}

/// Backend entry point: one step of a held press. See the module docs for
/// what to report and when.
pub fn report(phase: HeldPressPhase, x: f32, y: f32) {
    // Snapshot: a listener may add or drop listeners (picking a row closes
    // the menu, which drops every row's listener mid-dispatch).
    let snapshot: Vec<(Rc<Cell<bool>>, Handler)> =
        LISTENERS.with(|l| l.borrow().iter().map(|e| (e.alive.clone(), e.handler.clone())).collect());
    let event = HeldPressEvent { phase, x, y };
    // One reactive window for the whole dispatch, like every other input
    // channel: each row writes its lit state and effects run once.
    crate::cycle(|| {
        for (alive, handler) in snapshot {
            // Dropped earlier in this same dispatch: its scope is gone.
            if alive.get() {
                handler(&event);
            }
        }
    });
}

#[doc(hidden)]
pub fn reset_for_tests() {
    LISTENERS.with(|l| l.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listeners_hear_reports_until_dropped() {
        reset_for_tests();
        let seen: Rc<RefCell<Vec<HeldPressEvent>>> = Rc::default();
        let s = seen.clone();
        let guard = add_listener(move |e| s.borrow_mut().push(*e));
        report(HeldPressPhase::Moved, 3.0, 4.0);
        report(HeldPressPhase::Released, 5.0, 6.0);
        drop(guard);
        report(HeldPressPhase::Moved, 9.0, 9.0);
        assert_eq!(
            *seen.borrow(),
            vec![
                HeldPressEvent { phase: HeldPressPhase::Moved, x: 3.0, y: 4.0 },
                HeldPressEvent { phase: HeldPressPhase::Released, x: 5.0, y: 6.0 },
            ]
        );
    }

    /// Picking a row closes its menu, which drops every other row's
    /// listener while the release is still being dispatched. A listener
    /// dropped mid-dispatch must not run: its scope (signals, refs) is
    /// already torn down and a write into it panics.
    #[test]
    fn regression_a_listener_dropped_mid_dispatch_is_not_called() {
        reset_for_tests();
        let second_ran = Rc::new(Cell::new(false));
        let victim: Rc<RefCell<Option<HeldPressListener>>> = Rc::default();
        let v = victim.clone();
        let _first = add_listener(move |_| drop(v.borrow_mut().take()));
        let ran = second_ran.clone();
        *victim.borrow_mut() = Some(add_listener(move |_| ran.set(true)));
        report(HeldPressPhase::Released, 0.0, 0.0);
        assert!(!second_ran.get());
    }

    #[test]
    fn a_listener_may_register_another_during_dispatch() {
        reset_for_tests();
        let kept: Rc<RefCell<Vec<HeldPressListener>>> = Rc::default();
        let k = kept.clone();
        let _l = add_listener(move |_| k.borrow_mut().push(add_listener(|_| {})));
        report(HeldPressPhase::Moved, 0.0, 0.0);
        assert_eq!(kept.borrow().len(), 1);
    }
}
