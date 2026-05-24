//! Sidecar-side `runtime_core::scheduling::Scheduler` impl.
//!
//! The sidecar is a plain Rust process with no platform run loop
//! (no NSRunLoop, no browser raf, no Choreographer). Without an
//! installed `Scheduler`, `runtime_core::scheduling::raf_loop`
//! returns an inert handle and any author code using
//! `raf_loop_scoped` / `after_ms` / animation infrastructure silently
//! does nothing. That's how the welcome example's planets sat at
//! `opacity:0` even after we wired up the declarative animator path.
//!
//! Design choices:
//!
//! - **Process-global install, per-thread storage.** Schedulers in
//!   `runtime_core` live in a single `OnceLock<Box<dyn Scheduler>>`
//!   — first-install wins, all threads share. But the closures
//!   handed to `raf_loop` / `after_ms` aren't `Send` (they capture
//!   `!Send` user state — `Rc`s, recorder handles, …). So we install
//!   one [`SidecarScheduler`] unit struct, and each thread that calls
//!   `raf_loop` / `after_ms` stores its closure in a thread-local
//!   slot. The runtime-server sidecar runs one session per thread; closures
//!   stay local to the session that registered them.
//!
//! - **Client drives the cadence.** Each session thread's
//!   [`drive_pending`] runs on every `AppToDev::RequestFrame` from a
//!   client. So scheduler tick rate = client raf rate. Idle sessions
//!   pay nothing.
//!
//! - **Microtask = synchronous.** Native scheduler convention: no
//!   event loop to defer to, so microtasks just run inline at queue
//!   time. Same as `runtime_core::scheduling::schedule_microtask`'s
//!   built-in fallback on non-wasm targets.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use runtime_core::scheduling::{ScheduleHandle, Scheduler};

/// Unit struct because all per-callback state lives in thread-locals;
/// the struct itself carries nothing. `Send + Sync` falls out
/// trivially (no fields).
pub struct SidecarScheduler;

thread_local! {
    /// Recurring per-frame closures. Driven by [`drive_pending`].
    /// `Rc<RafSlot>` so a closure can re-enter the scheduler (cancel
    /// itself, register another raf, etc.) without invalidating the
    /// iterator — [`drive_pending`] snapshots the Rc list and releases
    /// the outer borrow before firing each callback. See [`RafSlot`]
    /// for why the closure isn't stored under a plain `RefCell`.
    static RAF_LOOPS: RefCell<Vec<Rc<RafSlot>>> =
        RefCell::new(Vec::new());

    /// One-shot deadlined closures (`after_ms`, `after_animation_frame`).
    /// Sorted by deadline only on insert is overkill; we just scan
    /// every drive and fire whatever's ready — fewer than ~100
    /// entries in practice (timeline events, transition tails).
    static DEADLINES: RefCell<Vec<DeadlineEntry>> =
        RefCell::new(Vec::new());
}

type RafFn = Box<dyn FnMut() + 'static>;

/// Storage for one raf-loop registration.
///
/// `closure` lives in a `Cell` rather than a `RefCell` because
/// [`drive_pending`] must execute the closure *without* holding any
/// borrow on this slot — the closure body almost always re-enters
/// the scheduler:
///
/// - `AV.set(...)` inside the body can fire a reactive cleanup that
///   drops this raf's handle ([`RafHandle::Drop`] → `cancel` →
///   touches the same slot).
/// - The closure may call `raf_loop_scoped(...)` itself to spawn a
///   follow-up loop.
///
/// With `RefCell` either of those re-entries would hit
/// `BorrowMutError`. With `Cell` + `take` + put-back, the slot is
/// physically empty during execution — re-entry sees `None` and
/// does nothing, which is exactly right.
///
/// `cancelled` distinguishes "we took the closure out for execution"
/// from "the handle was cancelled while running": after `f()`
/// returns, the put-back path checks the flag and drops `f` rather
/// than re-installing it if cancel won the race.
struct RafSlot {
    closure: Cell<Option<RafFn>>,
    cancelled: Cell<bool>,
}

struct DeadlineEntry {
    deadline: Instant,
    /// Inside `Option` so `cancel` can drop the closure without
    /// removing the entry (we'll lazily prune at next drive).
    closure: Rc<RefCell<Option<Box<dyn FnOnce() + 'static>>>>,
}

impl Scheduler for SidecarScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        // No event loop to defer to. Synchronous-now matches the
        // built-in `schedule_microtask` fallback for native targets
        // and is what every other dev-server code path assumes.
        f();
    }

    fn after_animation_frame(
        &self,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        // Treat "next animation frame" as ~16ms. Close enough for
        // the framework's frame-aligned scheduling needs; precise
        // raf alignment isn't a thing without a real display link.
        self.after_ms(16, f)
    }

    fn after_ms(
        &self,
        delay_ms: i32,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        let cell: Rc<RefCell<Option<Box<dyn FnOnce() + 'static>>>> =
            Rc::new(RefCell::new(Some(f)));
        DEADLINES.with(|d| {
            d.borrow_mut().push(DeadlineEntry {
                deadline: Instant::now()
                    + Duration::from_millis(delay_ms.max(0) as u64),
                closure: cell.clone(),
            });
        });
        Box::new(DeadlineHandle { cell })
    }

    fn raf_loop(&self, f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        let slot = Rc::new(RafSlot {
            closure: Cell::new(Some(f)),
            cancelled: Cell::new(false),
        });
        RAF_LOOPS.with(|r| {
            r.borrow_mut().push(slot.clone());
        });
        Box::new(RafHandle { slot })
    }
}

struct RafHandle {
    slot: Rc<RafSlot>,
}

impl ScheduleHandle for RafHandle {
    fn cancel(&mut self) {
        // Mark the slot dead BEFORE clearing the closure. drive_pending
        // checks `cancelled` after f() returns to decide whether to
        // re-install the FnMut it took out for execution — if cancel
        // raced the running closure, the flag tells the driver to
        // drop the FnMut instead of putting it back.
        self.slot.cancelled.set(true);
        // `Cell::set(None)` drops the previous contents (the FnMut,
        // freeing captures) without ever borrowing — safe to call
        // even if drive_pending is mid-execution of this same slot
        // (the slot is empty during execution; this is a no-op then).
        self.slot.closure.set(None);
    }
}

impl Drop for RafHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct DeadlineHandle {
    cell: Rc<RefCell<Option<Box<dyn FnOnce() + 'static>>>>,
}

impl ScheduleHandle for DeadlineHandle {
    fn cancel(&mut self) {
        *self.cell.borrow_mut() = None;
    }
}

impl Drop for DeadlineHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Install the sidecar scheduler. Idempotent — the underlying
/// `OnceLock` discards repeat installs, so this is safe to call
/// once per process at sidecar startup. Subsequent session-thread
/// spawns reuse the same install.
pub fn install() {
    runtime_core::scheduling::install_scheduler(Box::new(SidecarScheduler));
}

/// Drive everything the scheduler stashed on the **calling thread**:
/// fire expired deadlines, run every active raf-loop closure once.
/// The runtime-server sidecar's session thread calls this from
/// `WireRecordingBackend::tick_animations` on each `RequestFrame`.
///
/// Ordering matches browser convention: deadlines (timeouts) first,
/// raf_loops second. Microtasks are synchronous-at-queue-time so
/// they're already drained by the time we get here.
pub fn drive_pending() {
    let now = Instant::now();

    // 1. Expired deadlines. Drain ready entries in one pass; prune
    //    cancelled entries (closure dropped) opportunistically.
    let ready: Vec<Rc<RefCell<Option<Box<dyn FnOnce() + 'static>>>>> =
        DEADLINES.with(|d| {
            let mut deadlines = d.borrow_mut();
            let mut i = 0;
            let mut ready = Vec::new();
            while i < deadlines.len() {
                let entry = &deadlines[i];
                let cancelled = entry.closure.borrow().is_none();
                if cancelled || now >= entry.deadline {
                    let removed = deadlines.remove(i);
                    if !cancelled {
                        ready.push(removed.closure);
                    }
                } else {
                    i += 1;
                }
            }
            ready
        });
    for cell in ready {
        if let Some(f) = cell.borrow_mut().take() {
            f();
        }
    }

    // 2. Recurring raf_loops. Snapshot the Rc list so RAF_LOOPS
    //    itself isn't borrowed across user code — a closure that
    //    registers a new raf_loop must be able to push onto RAF_LOOPS.
    //
    //    Take the FnMut out of its slot (Cell::take, no borrow held)
    //    before calling it. The closure runs with no live borrow on
    //    the slot, so RAF_LOOP_HANDLE re-entries (handle cancels
    //    triggered by reactive cleanups inside the closure — see
    //    [`RafSlot`]) are safe. After the call: if `cancelled` was
    //    flipped during execution, drop the FnMut. Otherwise put it
    //    back for the next tick.
    let raf_snapshot: Vec<Rc<RafSlot>> = RAF_LOOPS.with(|r| r.borrow().clone());
    for slot in &raf_snapshot {
        if slot.cancelled.get() {
            continue;
        }
        let Some(mut f) = slot.closure.take() else {
            // Either pre-cancelled (handle dropped before drive arrived)
            // or another tick is somehow re-entering this slot; either
            // way, skip — Cell::take left None which is the right state.
            continue;
        };
        f();
        if slot.cancelled.get() {
            // Handle was cancelled during execution. Drop `f` (frees
            // its captures); the slot is already `None` from the take.
            drop(f);
        } else {
            slot.closure.set(Some(f));
        }
    }
    // Drop any cancelled entries from the live list. Cheap because
    // the common case is "no cancellations this tick."
    RAF_LOOPS.with(|r| {
        r.borrow_mut().retain(|s| !s.cancelled.get());
    });
}

#[cfg(test)]
mod tests {
    //! Regression coverage for the welcome-class scheduling pattern:
    //!
    //!   session::after_ms(at, body) {
    //!       raf_loop_scoped(per_frame);
    //!   }
    //!
    //! After hot-patch rerender, the session-relative `after_ms`
    //! computes `delay = 0` (elapsed already past `at`). The
    //! deadline must fire in the next [`drive_pending`] AND the
    //! `raf_loop_scoped` registered from inside it must actually
    //! produce frame callbacks on subsequent drives. The bug under
    //! investigation: in the live welcome scene, timeline-driven
    //! tweens flow after rerender but the raf_loop body never fires.
    use super::*;
    use runtime_core::scheduling::{after_ms_scoped, raf_loop_scoped};
    use std::cell::Cell;
    use std::rc::Rc;

    /// Install the sidecar scheduler exactly once across the whole
    /// test binary. `runtime_core::scheduling::install_scheduler`
    /// uses a `OnceLock`, so calling install repeatedly is harmless
    /// — but starting from a clean state per test isn't possible
    /// either. All scheduling tests therefore have to live with the
    /// install-once invariant.
    fn ensure_installed() {
        super::install();
    }

    /// Mirror of the welcome coordinator's pattern: schedule an
    /// `after_ms_scoped(delay, ...)` whose body calls
    /// `raf_loop_scoped(...)`. Drive once to fire the deadline +
    /// register the raf, then drive again to verify the raf body
    /// runs on the next frame.
    /// Exact welcome-coordinator pattern: an effect is constructed,
    /// its body schedules a `session::after_ms(at, raf_setup)`
    /// where `raf_setup` calls `raf_loop_scoped(per_frame)`. We
    /// drive long enough to pretend the session epoch has advanced
    /// past `at` (the post-rerender case), then drop the effect
    /// (mirroring `SessionMsg::Rerender`'s `owner.take()`) and
    /// rebuild a fresh one. The second pass must produce raf
    /// callbacks just like the first.
    #[test]
    fn rerender_re_registers_nested_raf_loop() {
        ensure_installed();

        // Helper: build one "lifetime" of the welcome pattern,
        // return an owning Effect + a per-lifetime call counter.
        fn build_lifetime() -> (runtime_core::Effect, Rc<Cell<u32>>) {
            let calls = Rc::new(Cell::new(0u32));
            let calls_for_body = calls.clone();
            let effect = runtime_core::Effect::new(move || {
                let counter = calls_for_body.clone();
                // delay=0 matches `session::after_ms(at, ...)` after
                // the session epoch has already passed `at`.
                after_ms_scoped(0, move || {
                    let counter_inner = counter.clone();
                    raf_loop_scoped(move || {
                        counter_inner.set(counter_inner.get() + 1);
                    });
                });
            });
            (effect, calls)
        }

        // First lifetime — like the initial mount.
        let (first_effect, first_calls) = build_lifetime();
        drive_pending();
        drive_pending();
        assert!(
            first_calls.get() >= 2,
            "first lifetime: raf should fire on each drive (got {})",
            first_calls.get()
        );

        // Drop the first effect (simulates Owner::drop on rerender).
        // Its on_cleanup chain MUST cancel the deadline + raf so the
        // second lifetime's counters are independent.
        drop(first_effect);
        let snapshot_after_drop = first_calls.get();
        drive_pending();
        assert_eq!(
            first_calls.get(),
            snapshot_after_drop,
            "first lifetime's raf must stop firing after its effect drops"
        );

        // Second lifetime — like the post-hot-patch mount.
        let (_second_effect, second_calls) = build_lifetime();
        drive_pending();
        drive_pending();
        assert!(
            second_calls.get() >= 2,
            "second lifetime (post-rerender): raf should fire on each drive (got {})",
            second_calls.get()
        );
    }

    #[test]
    fn deadline_then_nested_raf_loop_fires_on_next_drive() {
        ensure_installed();

        let raf_calls = Rc::new(Cell::new(0u32));
        let raf_calls_for_body = raf_calls.clone();
        let _effect = runtime_core::Effect::new(move || {
            let raf_calls_inner = raf_calls_for_body.clone();
            after_ms_scoped(0, move || {
                let counter = raf_calls_inner.clone();
                raf_loop_scoped(move || {
                    counter.set(counter.get() + 1);
                });
            });
        });

        // 1st drive: should fire the deadline (delay=0) which
        // schedules the raf. The raf body should also fire once
        // because the snapshot is taken AFTER deadlines run.
        drive_pending();
        let after_first = raf_calls.get();
        // 2nd drive: raf body should fire again.
        drive_pending();
        let after_second = raf_calls.get();

        assert!(
            after_first >= 1,
            "raf body should fire at least once on the drive that processes the deadline (got {})",
            after_first
        );
        assert!(
            after_second > after_first,
            "raf body should fire again on the next drive (first={}, second={})",
            after_first,
            after_second
        );
    }
}
