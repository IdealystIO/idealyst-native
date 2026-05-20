//! `on` / `on_defer` + `Trackable` — explicit-deps reactive APIs.
//!
//! `on(deps, callback)` takes an explicit dep set. The callback runs
//! once initially (or never, for `on_defer`) and re-runs on every
//! change to any dep. The callback receives `(&D::Value, Option<&D::Value>)`
//! — current value + the previous one (None on the first call).
//! Tuples of signals up to arity 4 are valid `Trackable` sets.

use std::cell::Cell;
use std::rc::Rc;

use framework_core::{on, on_defer, signal, Signal};

/// `on(signal, callback)` with a single signal: fires immediately
/// with `(initial, None)`, then `(new, Some(old))` per change.
#[test]
fn on_single_signal_fires_initial_plus_changes() {
    let s: Signal<i32> = signal!(0);
    let observed: Rc<Cell<Vec<(i32, Option<i32>)>>> = Rc::new(Cell::new(Vec::new()));
    let obs = observed.clone();

    let _e = on(s, move |v: &i32, prev: Option<&i32>| {
        let mut current = obs.take();
        current.push((*v, prev.copied()));
        obs.set(current);
    });

    let initial = observed.take();
    assert_eq!(initial, vec![(0, None)]);
    observed.set(initial);

    s.set(7);
    let after_one = observed.take();
    assert_eq!(after_one, vec![(0, None), (7, Some(0))]);
    observed.set(after_one);

    s.set(13);
    let after_two = observed.take();
    assert_eq!(
        after_two,
        vec![(0, None), (7, Some(0)), (13, Some(7))],
        "prev value carries forward"
    );
}

/// `on_defer(signal, callback)` skips the initial fire.
#[test]
fn on_defer_skips_initial_fires_on_change() {
    let s: Signal<i32> = signal!(0);
    let observed: Rc<Cell<Vec<i32>>> = Rc::new(Cell::new(Vec::new()));
    let obs = observed.clone();

    let _e = on_defer(s, move |v: &i32, _prev: Option<&i32>| {
        let mut current = obs.take();
        current.push(*v);
        obs.set(current);
    });

    let after_construct = observed.take();
    assert_eq!(after_construct, Vec::<i32>::new(), "on_defer skips initial");
    observed.set(after_construct);

    s.set(1);
    s.set(2);
    let after = observed.take();
    assert_eq!(after, vec![1, 2]);
}

/// `on((a, b), callback)`: 2-tuple deps, callback sees `&(i32, i32)`.
#[test]
fn on_tuple_arity_2() {
    let a: Signal<i32> = signal!(10);
    let b: Signal<i32> = signal!(20);
    let observed: Rc<Cell<Vec<(i32, i32)>>> = Rc::new(Cell::new(Vec::new()));
    let obs = observed.clone();

    let _e = on((a, b), move |v: &(i32, i32), _prev: Option<&(i32, i32)>| {
        let mut current = obs.take();
        current.push(*v);
        obs.set(current);
    });

    let after_init = observed.take();
    assert_eq!(after_init, vec![(10, 20)]);
    observed.set(after_init);

    a.set(11);
    b.set(21);
    let after = observed.take();
    assert_eq!(after, vec![(10, 20), (11, 20), (11, 21)]);
}

#[test]
fn on_tuple_arity_3() {
    let a: Signal<i32> = signal!(1);
    let b: Signal<i32> = signal!(2);
    let c: Signal<i32> = signal!(3);
    let count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ct = count.clone();

    let _e = on((a, b, c), move |_v: &(i32, i32, i32), _prev| {
        ct.set(ct.get() + 1);
    });

    assert_eq!(count.get(), 1);
    a.set(10);
    b.set(20);
    c.set(30);
    assert_eq!(count.get(), 4);
}

#[test]
fn on_tuple_arity_4() {
    let a: Signal<i32> = signal!(1);
    let b: Signal<i32> = signal!(2);
    let c: Signal<i32> = signal!(3);
    let d: Signal<i32> = signal!(4);
    let count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ct = count.clone();

    let _e = on((a, b, c, d), move |_v: &(i32, i32, i32, i32), _prev| {
        ct.set(ct.get() + 1);
    });

    assert_eq!(count.get(), 1);
    a.set(10);
    d.set(40);
    assert_eq!(count.get(), 3);
}

/// `on` reads only the declared deps. A signal NOT in the dep tuple
/// but read inside the callback is NOT subscribed.
#[test]
fn on_callback_reads_are_not_tracked() {
    let dep: Signal<i32> = signal!(0);
    let other: Signal<i32> = signal!(0);
    let count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ct = count.clone();

    let _e = on(dep, move |_v: &i32, _prev| {
        let _ = other.get();
        ct.set(ct.get() + 1);
    });

    assert_eq!(count.get(), 1);
    other.set(99);
    assert_eq!(count.get(), 1, "callback's read didn't subscribe");

    dep.set(1);
    assert_eq!(count.get(), 2);
}

/// Dropping the returned Effect handle stops further fires.
#[test]
fn on_handle_drop_stops_firing() {
    let s: Signal<i32> = signal!(0);
    let count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ct = count.clone();

    let e = on(s, move |_v: &i32, _prev| {
        ct.set(ct.get() + 1);
    });

    assert_eq!(count.get(), 1);
    s.set(1);
    assert_eq!(count.get(), 2);

    drop(e);

    s.set(2);
    assert_eq!(count.get(), 2);
}

#[test]
fn on_defer_with_tuple_deps() {
    let a: Signal<i32> = signal!(0);
    let b: Signal<i32> = signal!(0);
    let count: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let ct = count.clone();

    let _e = on_defer((a, b), move |_v: &(i32, i32), _prev| {
        ct.set(ct.get() + 1);
    });

    assert_eq!(count.get(), 0);
    a.set(1);
    b.set(1);
    assert_eq!(count.get(), 2);
}
