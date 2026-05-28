//! Re-entrancy inside `Signal::update` / `Signal::set` closures.
//!
//! `update`/`set` run their closure via `with_signal_mut`, which used to
//! hold the signal-arena `RefCell` borrow across the closure. Any signal
//! access inside — `Signal::new`, another signal's `.get()` — re-enters
//! the arena and panicked: `RefCell already borrowed` (reactive.rs:979).
//!
//! The fix takes the signal's box out of the arena and drops the borrow
//! before running the closure, then restores it. These tests pin that:
//! they panic against the pre-fix code and pass after.
//!
//! Real-world trigger: the `reactive-loops` demo's "Add item" handler
//! pushes a `Row { count: signal!(0) }` inside `items.update(|l| …)`.

use runtime_core::{signal, Signal};

/// Allocating a new signal inside an `update` closure must not panic,
/// and the nested signal must be live + independent afterward.
#[test]
fn create_signal_inside_update_closure() {
    let items: Signal<Vec<Signal<i32>>> = signal!(Vec::new());

    items.update(|v| {
        // `Signal::new` re-enters the arena — the regression.
        let child = Signal::new(0);
        v.push(child);
    });

    assert_eq!(items.get().len(), 1, "the pushed row should be present");

    // The nested signal works on its own (it wasn't corrupted by the
    // take/restore of the outer signal's slot).
    let child = items.get()[0];
    assert_eq!(child.get(), 0);
    child.set(42);
    assert_eq!(child.get(), 42);

    // The outer signal is intact and reads the updated child.
    assert_eq!(items.get()[0].get(), 42);
}

/// Reading a *different* signal inside an `update` closure is also fine
/// (same re-entrant arena access, read path).
#[test]
fn read_other_signal_inside_update_closure() {
    let a: Signal<i32> = signal!(10);
    let b: Signal<i32> = signal!(0);

    b.update(|v| {
        *v = a.get() + 1;
    });

    assert_eq!(b.get(), 11);
    assert_eq!(a.get(), 10, "reading `a` must not disturb it");
}

/// Several rows, each pushed with its own freshly-allocated signal inside
/// successive `update` calls — the `reactive-loops` "Add item" loop.
#[test]
fn repeated_add_with_nested_signal() {
    let items: Signal<Vec<Signal<i32>>> = signal!(Vec::new());

    for start in 0..5 {
        // Allocate INSIDE the update closure — the re-entrant case.
        items.update(move |l| l.push(signal!(start)));
    }

    let snapshot: Vec<i32> = items.get().iter().map(|s| s.get()).collect();
    assert_eq!(snapshot, vec![0, 1, 2, 3, 4]);
}
