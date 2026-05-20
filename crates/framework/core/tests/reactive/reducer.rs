//! `reducer(initial, |state, action| next)` — action-driven state.
//!
//! Returns `(Signal<S>, impl Fn(A))`: a read-only signal of the
//! current state, and a dispatch function that consumes one action.
//! The reducer closure runs untracked — `dispatch` from inside an
//! effect doesn't subscribe that effect to the state.

use std::cell::Cell;
use std::rc::Rc;

use framework_core::{reducer, Effect};

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Inc,
    Dec,
    Add(i32),
    Reset,
}

/// Basic dispatch → state transition → signal update.
#[test]
fn dispatch_updates_state() {
    let (state, dispatch) = reducer(0i32, |s, a: Action| match a {
        Action::Inc => s + 1,
        Action::Dec => s - 1,
        Action::Add(n) => s + n,
        Action::Reset => 0,
    });

    assert_eq!(state.get(), 0);
    dispatch(Action::Inc);
    assert_eq!(state.get(), 1);
    dispatch(Action::Add(10));
    assert_eq!(state.get(), 11);
    dispatch(Action::Dec);
    assert_eq!(state.get(), 10);
    dispatch(Action::Reset);
    assert_eq!(state.get(), 0);
}

/// Subscribers to the state signal fire on each successful transition.
#[test]
fn state_signal_notifies_subscribers() {
    let (state, dispatch) = reducer(0i32, |s, _a: Action| s + 1);
    let count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ct = count.clone();

    let _e = Effect::new(move || {
        let _ = state.get();
        ct.set(ct.get() + 1);
    });

    assert_eq!(count.get(), 1, "initial");
    dispatch(Action::Inc);
    dispatch(Action::Inc);
    dispatch(Action::Inc);
    assert_eq!(count.get(), 4, "one fire per dispatch");
}

/// Dispatching from inside an Effect does NOT subscribe the effect
/// to its own state — the reducer's read of state is untracked.
/// Without this, every dispatch-from-effect would re-fire the effect
/// in a loop.
#[test]
fn dispatch_from_effect_does_not_self_subscribe() {
    let (state, dispatch) = reducer(0i32, |s, _a: Action| s + 1);
    let count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ct = count.clone();

    let dispatched: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let dispatched_for_effect = dispatched.clone();

    let _e = Effect::new(move || {
        ct.set(ct.get() + 1);
        // Dispatch exactly once on the initial run.
        if !dispatched_for_effect.get() {
            dispatched_for_effect.set(true);
            dispatch(Action::Inc);
        }
    });

    // Effect ran once for initial; dispatch happened during that run.
    // The dispatch caused state.set() but the effect doesn't read
    // state, so no re-fire.
    assert_eq!(count.get(), 1, "no infinite loop");
    assert_eq!(state.get(), 1, "state was updated");
}

/// Reducer closure can read other signals — those become deps... of
/// nothing in particular, because the reducer body itself runs
/// outside any effect (it's just a regular fn invocation from
/// dispatch). Reads inside the reducer are NOT tracked.
#[test]
fn reducer_body_reads_are_not_tracked() {
    use framework_core::{signal, Signal};
    let modifier: Signal<i32> = signal!(10);

    let (state, dispatch) = reducer(0i32, move |s, _a: Action| s + modifier.get());

    // Initial state read.
    let count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ct = count.clone();
    let _e = Effect::new(move || {
        let _ = state.get();
        ct.set(ct.get() + 1);
    });

    assert_eq!(count.get(), 1);

    dispatch(Action::Inc);
    assert_eq!(state.get(), 10);
    assert_eq!(count.get(), 2);

    // Changing modifier doesn't trigger the state-subscriber.
    modifier.set(100);
    assert_eq!(
        count.get(),
        2,
        "modifier is not in the dep set of state-subscriber effect"
    );

    dispatch(Action::Inc);
    assert_eq!(state.get(), 110);
    assert_eq!(count.get(), 3);
}
