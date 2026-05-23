//! Sidecar-side `framework_core::scheduling::Scheduler` impl.
//!
//! The sidecar is a plain Rust process with no platform run loop
//! (no NSRunLoop, no browser raf, no Choreographer). Without an
//! installed `Scheduler`, `framework_core::scheduling::raf_loop`
//! returns an inert handle and any author code using
//! `raf_loop_scoped` / `after_ms` / animation infrastructure silently
//! does nothing. That's how the welcome example's planets sat at
//! `opacity:0` even after we wired up the declarative animator path.
//!
//! Design choices:
//!
//! - **Process-global install, per-thread storage.** Schedulers in
//!   `framework_core` live in a single `OnceLock<Box<dyn Scheduler>>`
//!   — first-install wins, all threads share. But the closures
//!   handed to `raf_loop` / `after_ms` aren't `Send` (they capture
//!   `!Send` user state — `Rc`s, recorder handles, …). So we install
//!   one [`SidecarScheduler`] unit struct, and each thread that calls
//!   `raf_loop` / `after_ms` stores its closure in a thread-local
//!   slot. The AAS sidecar runs one session per thread; closures
//!   stay local to the session that registered them.
//!
//! - **Client drives the cadence.** Each session thread's
//!   [`drive_pending`] runs on every `AppToDev::RequestFrame` from a
//!   client. So scheduler tick rate = client raf rate. Idle sessions
//!   pay nothing.
//!
//! - **Microtask = synchronous.** Native scheduler convention: no
//!   event loop to defer to, so microtasks just run inline at queue
//!   time. Same as `framework_core::scheduling::schedule_microtask`'s
//!   built-in fallback on non-wasm targets.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use framework_core::scheduling::{ScheduleHandle, Scheduler};

/// Unit struct because all per-callback state lives in thread-locals;
/// the struct itself carries nothing. `Send + Sync` falls out
/// trivially (no fields).
pub struct SidecarScheduler;

thread_local! {
    /// Recurring per-frame closures. Driven by [`drive_pending`].
    /// `Rc<RefCell<…>>` rather than the bare closure so a closure
    /// can `cancel` itself mid-tick without invalidating the iterator
    /// — [`drive_pending`] snapshots the Rc list, releases the outer
    /// borrow, then fires each callback. Inner `Option` lets `cancel`
    /// drop the closure eagerly (frees captured state) without
    /// disturbing the slot's address (so the eq-by-Rc-ptr removal
    /// stays valid).
    static RAF_LOOPS: RefCell<Vec<Rc<RefCell<Option<RafFn>>>>> =
        RefCell::new(Vec::new());

    /// One-shot deadlined closures (`after_ms`, `after_animation_frame`).
    /// Sorted by deadline only on insert is overkill; we just scan
    /// every drive and fire whatever's ready — fewer than ~100
    /// entries in practice (timeline events, transition tails).
    static DEADLINES: RefCell<Vec<DeadlineEntry>> =
        RefCell::new(Vec::new());
}

type RafFn = Box<dyn FnMut() + 'static>;

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
        let cell: Rc<RefCell<Option<RafFn>>> = Rc::new(RefCell::new(Some(f)));
        RAF_LOOPS.with(|r| {
            r.borrow_mut().push(cell.clone());
        });
        Box::new(RafHandle { cell })
    }
}

struct RafHandle {
    cell: Rc<RefCell<Option<RafFn>>>,
}

impl ScheduleHandle for RafHandle {
    fn cancel(&mut self) {
        // Drop the closure (frees its captures). The empty slot
        // gets pruned on the next [`drive_pending`].
        *self.cell.borrow_mut() = None;
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
    framework_core::scheduling::install_scheduler(Box::new(SidecarScheduler));
}

/// Drive everything the scheduler stashed on the **calling thread**:
/// fire expired deadlines, run every active raf-loop closure once.
/// The AAS sidecar's session thread calls this from
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

    // 2. Recurring raf_loops. Snapshot the Rc list so the borrow is
    //    released before we fire the closures — a closure that
    //    registers a new raf_loop (rare but legal) won't trip
    //    BorrowMutError. Prune any slots whose closure was cancelled
    //    while we weren't holding the borrow.
    let raf_snapshot: Vec<Rc<RefCell<Option<RafFn>>>> =
        RAF_LOOPS.with(|r| r.borrow().clone());
    for cell in &raf_snapshot {
        // Re-check `is_none` against drop-while-iterating; cell may
        // have been cancelled by a previous callback in this pass.
        let mut slot = cell.borrow_mut();
        if let Some(f) = slot.as_mut() {
            f();
        }
    }
    // Drop any cancelled entries from the live list. Cheap because
    // the common case is "no cancellations this tick."
    RAF_LOOPS.with(|r| {
        r.borrow_mut().retain(|c| c.borrow().is_some());
    });
}
