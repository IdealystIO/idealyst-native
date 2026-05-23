//! Regression: a raf closure that causes its own `RafLoop` handle to
//! drop mid-execution must NOT panic with `RefCell already borrowed`.
//!
//! The bug: [`SidecarScheduler`] originally held a `RefCell::borrow_mut`
//! on the slot's `Option<RafFn>` for the entire duration of the
//! closure call. If anything inside that call (a reactive cleanup
//! firing, an AV.set triggering effect re-runs, …) caused the
//! corresponding `RafHandle` to drop, `RafHandle::cancel`'s
//! `cell.borrow_mut() = None` re-entered the same RefCell and panicked.
//!
//! Real-world trigger: welcome's
//! [`coordinator::use_welcome`](../../../examples/welcome/src/coordinator.rs)
//! schedules a `raf_loop_scoped(...)` inside an `after_ms_scoped(...)`
//! inside an `effect!(...)`. When AV writes inside the raf body
//! invalidate the outer effect, the effect re-runs, the child scope
//! cleanups fire, the raf handle drops — and we crash.
//!
//! This test reproduces the cancel-during-execution shape without
//! pulling in the full reactive system: the raf closure itself owns
//! the handle and drops it on the first tick. Without the fix the
//! test panics inside `drive_pending`; with the fix the closure runs
//! cleanly, the handle drops, and the slot prunes on the next pass.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use dev_server::scheduler;
use framework_core::scheduling::{raf_loop, RafLoop};

/// Helper: install the scheduler once per test binary. The framework's
/// `install_scheduler` is `OnceLock`-guarded so repeated calls within
/// a single test binary are no-ops; integration test files compile as
/// separate binaries so this doesn't leak across files.
fn install_once() {
    scheduler::install();
}

#[test]
fn regression_raf_closure_dropping_own_handle_does_not_panic() {
    install_once();

    let ticks = Rc::new(Cell::new(0u32));
    let ticks_for_closure = ticks.clone();

    // Stash the raf handle in a Cell that the closure can take from.
    // The closure increments the tick counter, then drops the handle —
    // which under the old scheduler would re-enter the slot's RefCell
    // and panic.
    let handle_slot: Rc<RefCell<Option<RafLoop>>> = Rc::new(RefCell::new(None));
    let handle_slot_for_closure = handle_slot.clone();

    let handle = raf_loop(move || {
        ticks_for_closure.set(ticks_for_closure.get() + 1);
        // Take the handle out and drop it. `RafLoop::Drop` →
        // `RafHandle::Drop` → `cancel()`. With the old scheduler this
        // would attempt `cell.borrow_mut()` on the slot we're currently
        // executing inside → `RefCell already borrowed` panic.
        if let Some(h) = handle_slot_for_closure.borrow_mut().take() {
            drop(h);
        }
    });
    *handle_slot.borrow_mut() = Some(handle);

    // First drive: closure runs, increments counter, drops its own
    // handle. Must not panic.
    scheduler::drive_pending();
    assert_eq!(ticks.get(), 1, "raf closure must have fired once");

    // Second drive: the handle was cancelled during the first tick,
    // so the slot should be pruned and the closure must NOT fire again.
    scheduler::drive_pending();
    assert_eq!(
        ticks.get(),
        1,
        "cancelled raf must not tick again on subsequent drives"
    );
}

/// Sanity test: a raf closure that does NOT cancel itself keeps
/// ticking across `drive_pending` calls. Guards against the fix
/// over-correcting and eagerly pruning live slots.
#[test]
fn live_raf_closure_continues_ticking_across_drives() {
    install_once();

    let ticks = Rc::new(Cell::new(0u32));
    let ticks_for_closure = ticks.clone();

    let _handle = raf_loop(move || {
        ticks_for_closure.set(ticks_for_closure.get() + 1);
    });

    for expected in 1..=5 {
        scheduler::drive_pending();
        assert_eq!(ticks.get(), expected);
    }
}
