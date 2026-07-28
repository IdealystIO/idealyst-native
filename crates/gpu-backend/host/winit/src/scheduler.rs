//! Native scheduler for the wgpu sim runtime.
//!
//! The framework's `runtime_core::scheduling` helpers
//! (`after_ms`, `raf_loop`, `schedule_microtask`) all dispatch
//! through a single installed [`Scheduler`]. Without one:
//! - `after_ms` runs *synchronously* (delay ignored), so a timeline
//!   that schedules act 1 at +400 ms fires before mount returns
//!   and every act collapses to the same instant.
//! - `raf_loop` is INERT — the registered closure never fires, so
//!   `AnimatedValue`s never tick (the clock's tick driver is a
//!   `raf_loop`), and per-frame author code (welcome's sun/planet
//!   pulse) never runs.
//!
//! On mobile, `backend-ios-core` / `backend-android` install
//! NSTimer / Handler-based schedulers. The sim runtime had no
//! native equivalent, which is why every author-driven animation
//! silently froze on `idealyst run sim`.
//!
//! # Design
//!
//! - **Closures live on the main thread** (`MAIN_QUEUE`
//!   thread-local). The `Send + Sync` trait bound on `Scheduler`
//!   would otherwise force the closures themselves to be `Send`,
//!   which isn't representable for the framework's `FnOnce` /
//!   `FnMut` builders that capture `Rc` state.
//! - **A single worker thread** holds a min-heap of deadlines and
//!   sleeps until the next one. On fire it sends a wake event via
//!   the winit `EventLoopProxy<AppEvent>`; the main thread's
//!   `user_event` handler calls back into [`drain_due`] to run
//!   any callbacks whose deadlines have passed.
//! - **`raf_loop` clients** are stored in a parallel
//!   `Vec<RafEntry>` in `MAIN_QUEUE`. The worker thread emits a
//!   16 ms pulse whenever at least one entry is alive; pulses
//!   become `AppEvent::Tick` and the main thread drains every
//!   `raf` closure in order.
//!
//! Cancellation is cooperative: handle `Drop` removes the entry
//! from `MAIN_QUEUE`; the worker discovers the absence on the
//! next wake and skips it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use winit::event_loop::EventLoopProxy;

use crate::app::AppEvent;

/// Commands the main thread sends to the worker.
enum WorkerCmd {
    /// Register a new one-shot timer with the given absolute
    /// deadline. The id matches an entry already inserted in
    /// `MAIN_QUEUE.timers`.
    AfterMs { id: u64, deadline: Instant },
    /// Ensure the worker's 60 Hz raf pulse is active. Idempotent —
    /// the worker tracks pulse state itself.
    EnableRaf,
    /// Hint that no raf clients are alive (the last one just
    /// dropped). The worker stops emitting pulses on the next
    /// loop iteration. Live state on the main thread is the
    /// authority; this is purely an optimization.
    DisableRaf,
}

/// One pending one-shot timer. The deadline is duplicated here
/// (the worker also tracks it) so the main-thread drain step
/// fires only what's actually due — a single `SchedTick` event
/// may arrive ahead of N still-future timers when at least one
/// has expired.
struct PendingTimer {
    f: Option<Box<dyn FnOnce() + 'static>>,
    deadline: Instant,
}

/// One active raf-loop client. `alive` is shared with the matching
/// [`RafHandle`] so cancellation can flip the entry off without
/// touching `MAIN_QUEUE.rafs` directly — which matters because
/// [`drain_due`] temporarily `mem::take`s that Vec while ticking,
/// so a cancel that fires mid-tick (the AV clock's `raf_handle =
/// None` on settle) would otherwise find an empty MAIN_QUEUE and
/// silently lose its retain.
struct RafEntry {
    id: u64,
    f: Box<dyn FnMut() + 'static>,
    alive: Rc<Cell<bool>>,
}

/// Per-thread state. Closures live here so they don't have to be
/// `Send`. Only the main thread mutates this; the worker only
/// signals time via the event-loop proxy.
struct MainQueue {
    timers: HashMap<u64, PendingTimer>,
    rafs: Vec<RafEntry>,
}

thread_local! {
    static MAIN_QUEUE: RefCell<MainQueue> = RefCell::new(MainQueue {
        timers: HashMap::new(),
        rafs: Vec::new(),
    });
}

/// Worker's sender, set once at [`install`] and reused by every
/// scheduler call. Held behind a `Mutex<Option<…>>` so the static
/// `Scheduler` impl can clone it on demand without runtime
/// `OnceLock`-from-multi-thread gymnastics.
static CMD_TX: Mutex<Option<Sender<WorkerCmd>>> = Mutex::new(None);

/// Monotonic id allocator for both timer and raf entries.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Live raf-client count. The worker keeps emitting 60 Hz pulses
/// while this is > 0; on `0 → 1` we send `EnableRaf`, on `1 → 0`
/// we send `DisableRaf`. Tracking this separately from
/// `MAIN_QUEUE.rafs.len()` avoids the bug where a `RafHandle`'s
/// `Drop` fires mid-`drain_due` (when `MAIN_QUEUE.rafs` has been
/// `mem::take`-d into a local) and would otherwise observe an
/// empty Vec, send a spurious `DisableRaf`, and stall any other
/// raf clients still running — notably the welcome page's
/// forever-pulse driver, which is what tipped this over.
static RAF_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Install the winit-host scheduler. Called once from `run()`
/// BEFORE the user's `build_ui` mounts and starts dispatching
/// `after_ms` / `raf_loop`. `proxy` is the event-loop's proxy —
/// the worker thread uses it to wake the main thread.
///
/// Idempotent at the framework level (the inner `install_scheduler`
/// uses a `OnceLock`); this function also short-circuits if the
/// worker is already running.
pub(crate) fn install(proxy: EventLoopProxy<AppEvent>) {
    {
        let mut slot = CMD_TX.lock().unwrap();
        if slot.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel::<WorkerCmd>();
        *slot = Some(tx);
        thread::Builder::new()
            .name("idealyst-sim-scheduler".to_string())
            .spawn(move || worker_main(rx, proxy))
            .expect("spawn scheduler worker");
    }
    install_scheduler(Box::new(WinitScheduler));
}

/// Worker thread entry point. Maintains a sorted list of
/// `(deadline, id)` pairs and a `raf_active` flag; sleeps until
/// either the next timer deadline or the next raf pulse, whichever
/// is sooner, then signals the main thread.
///
/// The worker doesn't hold the closures — those live on the main
/// thread. On a missed deadline (e.g. the closure was cancelled
/// before fire), the main thread's drain step is a no-op.
fn worker_main(rx: mpsc::Receiver<WorkerCmd>, proxy: EventLoopProxy<AppEvent>) {
    /// Approximate animation-frame cadence. Real displays vary
    /// (60 / 90 / 120 Hz), but the framework's tick clamps `dt`
    /// internally so over-/under-shoot a few ms is fine.
    const RAF_PERIOD: Duration = Duration::from_millis(16);

    let mut timers: Vec<(Instant, u64)> = Vec::new();
    let mut raf_active = false;
    let mut next_raf = Instant::now();

    loop {
        // Drain any pending commands without blocking.
        loop {
            match rx.try_recv() {
                Ok(WorkerCmd::AfterMs { id, deadline }) => {
                    timers.push((deadline, id));
                    timers.sort_by_key(|(d, _)| *d);
                }
                Ok(WorkerCmd::EnableRaf) => {
                    if !raf_active {
                        raf_active = true;
                        next_raf = Instant::now();
                    }
                }
                Ok(WorkerCmd::DisableRaf) => raf_active = false,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        let now = Instant::now();

        // Fire every expired timer in a single batch.
        let mut fired_any = false;
        while let Some(&(deadline, _)) = timers.first() {
            if deadline > now {
                break;
            }
            timers.remove(0);
            fired_any = true;
        }
        if fired_any {
            // Tell the main thread to drain due timers from
            // MAIN_QUEUE — we have no closure to send, just a
            // wake signal.
            let _ = proxy.send_event(AppEvent::SchedTick);
        }

        // Raf pulse: send one wake per RAF_PERIOD while active.
        if raf_active && now >= next_raf {
            let _ = proxy.send_event(AppEvent::SchedTick);
            next_raf = now + RAF_PERIOD;
        }

        // Compute next wake. min(next_timer, next_raf if active).
        // If neither, block on the channel until a command arrives.
        let next_wake = match (timers.first().map(|(d, _)| *d), raf_active) {
            (Some(t_d), true) => Some(t_d.min(next_raf)),
            (Some(t_d), false) => Some(t_d),
            (None, true) => Some(next_raf),
            (None, false) => None,
        };
        match next_wake {
            Some(deadline) => {
                let sleep_for = deadline.saturating_duration_since(Instant::now());
                if sleep_for > Duration::ZERO {
                    match rx.recv_timeout(sleep_for) {
                        Ok(cmd) => requeue(cmd, &mut timers, &mut raf_active, &mut next_raf),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
            }
            None => {
                match rx.recv() {
                    Ok(cmd) => requeue(cmd, &mut timers, &mut raf_active, &mut next_raf),
                    Err(_) => return,
                }
            }
        }
    }
}

/// Apply a worker command received via blocking `recv` (where we
/// can't drop back into the try_recv drain loop without an extra
/// branch). Same effect as the loop body — kept out-of-line so
/// the worker's main loop is readable.
fn requeue(
    cmd: WorkerCmd,
    timers: &mut Vec<(Instant, u64)>,
    raf_active: &mut bool,
    next_raf: &mut Instant,
) {
    match cmd {
        WorkerCmd::AfterMs { id, deadline } => {
            timers.push((deadline, id));
            timers.sort_by_key(|(d, _)| *d);
        }
        WorkerCmd::EnableRaf => {
            if !*raf_active {
                *raf_active = true;
                *next_raf = Instant::now();
            }
        }
        WorkerCmd::DisableRaf => *raf_active = false,
    }
}

/// Drain every timer whose deadline has passed and run its
/// closure. Called from the winit `user_event` handler on
/// `AppEvent::SchedTick`.
pub(crate) fn drain_due() {
    let now = Instant::now();
    // Move every due `FnOnce` out of `MAIN_QUEUE` under a short
    // borrow so callbacks that re-enter `after_ms` (or anything
    // else that takes `MAIN_QUEUE`) don't trip the RefCell.
    let to_fire: Vec<Box<dyn FnOnce() + 'static>> = MAIN_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        // Drain only timers whose deadline has actually passed.
        // Without this filter every `SchedTick` event would fire
        // every still-future timer in the registry, collapsing
        // the welcome's three-act timeline (and any other
        // multi-deadline schedule) into a single frame.
        let due_ids: Vec<u64> = q
            .timers
            .iter()
            .filter_map(|(id, t)| (t.deadline <= now && t.f.is_some()).then_some(*id))
            .collect();
        let mut out = Vec::with_capacity(due_ids.len());
        for id in due_ids {
            if let Some(mut pt) = q.timers.remove(&id) {
                if let Some(f) = pt.f.take() {
                    out.push(f);
                }
            }
        }
        out
    });
    for f in to_fire {
        f();
    }
    // Tick every active raf client. The closure is `FnMut`, so we
    // need a mutable borrow per call — but a borrow held across
    // every closure would prevent the closure from registering new
    // rafs. We swap-out the Vec, tick the locals, then swap back
    // any survivors.
    //
    // Mid-tick cancellation is reported via `entry.alive` (set to
    // `false` by `RafHandle::cancel`). We skip dead entries in the
    // tick loop AND drop them during the merge — without that
    // filter, the AV clock's `c.raf_handle = None` (which fires
    // when an animation settles, AND can fire mid-`drain_due`)
    // would re-introduce the just-cancelled entry into
    // `MAIN_QUEUE.rafs`, leaving zombie tick closures that
    // accumulate every animation cycle.
    let mut taken: Vec<RafEntry> =
        MAIN_QUEUE.with(|q| std::mem::take(&mut q.borrow_mut().rafs));
    for entry in taken.iter_mut() {
        if entry.alive.get() {
            (entry.f)();
        }
    }
    MAIN_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        // Drop dead entries before merging; any entries that
        // (re-)registered during the tick are already in
        // `q.rafs` and stay there.
        taken.retain(|e| e.alive.get());
        let mut merged = taken;
        merged.append(&mut q.rafs);
        q.rafs = merged;
    });
}

/// Public scheduler type stored inside the framework's
/// `install_scheduler` slot. Zero-sized; all live state is in
/// `MAIN_QUEUE` (per-thread closures) + `CMD_TX` (worker channel).
struct WinitScheduler;

// SAFETY: see `IosScheduler`'s rationale. We hold no shared
// state on the struct itself — `CMD_TX` is a `Mutex`-guarded
// global, and the closures live in `thread_local` storage. The
// `Send + Sync` bound is satisfied by the empty struct alone.
unsafe impl Send for WinitScheduler {}
unsafe impl Sync for WinitScheduler {}

/// Register a one-shot timer WITHOUT the new-core post-dispatch hook
/// wrap. Shared registration body for [`Scheduler::after_ms`] (which
/// wraps) and [`Scheduler::schedule_microtask`] (which must NOT wrap —
/// see the comment there).
fn after_ms_raw(delay_ms: i32, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let deadline = Instant::now() + Duration::from_millis(delay_ms.max(0) as u64);
    MAIN_QUEUE.with(|q| {
        q.borrow_mut().timers.insert(
            id,
            PendingTimer { f: Some(f), deadline },
        );
    });
    if let Some(tx) = CMD_TX.lock().unwrap().clone() {
        let _ = tx.send(WorkerCmd::AfterMs { id, deadline });
    }
    Box::new(TimerHandle { id })
}

impl Scheduler for WinitScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        // "Microtask" = run after the current synchronous stack
        // unwinds, on the same thread. Implementing it as a 0 ms
        // `after_ms` lands the closure in the next event-loop
        // iteration — same shape as iOS's NSTimer-based scheduler.
        //
        // Routed through `after_ms_raw`, NOT `after_ms`: microtasks
        // must not fire the new-core post-dispatch hook. The new-core
        // flush itself is dispatched as a scheduled microtask, so
        // hooking here would re-schedule a flush from inside the
        // flush's own dispatch and spin the 0 ms timer queue forever
        // (see `render_wgpu::dispatch_hook`'s module docs and the
        // Android scheduler's identical exclusion). No author code
        // reaches the microtask queue outside an already-hooked
        // surface (wrapped event callbacks, timers, frames).
        //
        // `forget` the returned handle because microtasks are
        // fire-and-forget by contract: dropping the handle here
        // would cancel the timer before the worker ever wakes
        // (the framework discards the return value, so we'd
        // otherwise be cancelling a microtask scheduled milli-
        // seconds ago).
        std::mem::forget(after_ms_raw(0, f));
    }

    fn after_animation_frame(
        &self,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        // Match the rest of the framework's scheduler impls — one
        // animation frame ≈ 16 ms. The worker may signal sooner
        // if a timer is due before the next raf pulse; either way
        // the closure fires once. Routes through `after_ms`, so the
        // post-dispatch hook wrap below covers frame one-shots too.
        self.after_ms(16, f)
    }

    fn after_ms(
        &self,
        delay_ms: i32,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        // New-core flush driver: `after_ms` timers run author code (a
        // debounce that sets a signal); fire the post-dispatch hook
        // after the callback so staged writes commit. A single
        // thread-local read when no hook is installed (old core).
        // Wrapped HERE (at the Scheduler impl) rather than in
        // `drain_due` so `schedule_microtask`'s 0 ms timers stay
        // unhooked — the flush-microtask re-arm trap above.
        let f = Box::new(move || {
            f();
            render_wgpu::dispatch_hook::fire_dispatch_hook();
        });
        after_ms_raw(delay_ms, f)
    }

    fn raf_loop(&self, f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let alive = Rc::new(Cell::new(true));
        // Post-dispatch hook per iteration (animation ticks that stage
        // writes — e.g. an author `raf_loop` driving a signal). Same
        // rationale as `after_ms`; deduped downstream, so a running
        // loop costs at most one queued flush per drain.
        let f = {
            let mut f = f;
            Box::new(move || {
                f();
                render_wgpu::dispatch_hook::fire_dispatch_hook();
            })
        };
        MAIN_QUEUE.with(|q| {
            q.borrow_mut().rafs.push(RafEntry {
                id,
                f,
                alive: alive.clone(),
            });
        });
        // Bump the live-raf count first; if we were the first client,
        // signal the worker to start pulsing. The strict ordering
        // matters when two clients register in quick succession —
        // a fetch_add ≥ 1 by another thread before our send would
        // also have signalled, but Worker treats EnableRaf as
        // idempotent so double-signal is fine.
        let prev = RAF_COUNT.fetch_add(1, Ordering::SeqCst);
        if prev == 0 {
            if let Some(tx) = CMD_TX.lock().unwrap().clone() {
                let _ = tx.send(WorkerCmd::EnableRaf);
            }
        }
        Box::new(RafHandle { id, alive })
    }
}

/// Handle returned from `after_ms` / `schedule_microtask` /
/// `after_animation_frame`. `Drop` removes the closure from
/// `MAIN_QUEUE.timers`; the worker keeps the deadline in its
/// own list but the main-thread drain step skips it because
/// the slot is gone.
struct TimerHandle {
    id: u64,
}

impl ScheduleHandle for TimerHandle {
    fn cancel(&mut self) {
        // `try_with`, NOT `with`: handles can drop during THREAD
        // TEARDOWN, after `MAIN_QUEUE`'s own destructor ran. Concretely:
        // `std::process::exit` on macOS runs the main thread's TLS
        // destructors, `runtime_core::scheduling::DETACHED_TASKS` drops
        // its parked `ScheduledTask`s (cancel-on-drop), and a plain
        // `with` here aborts the whole exit with "cannot access a TLS
        // value during or after destruction" → "panic in a destructor
        // during cleanup". A destroyed queue needs no cleanup — skip.
        // (app.rs's windowWillClose `_exit` comment describes the same
        // trap; this is the root fix so normal `process::exit` works.)
        let _ = MAIN_QUEUE.try_with(|q| {
            q.borrow_mut().timers.remove(&self.id);
        });
    }
}

impl Drop for TimerHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Handle returned from `raf_loop`. `Drop` flips the shared
/// `alive` flag to `false` so the matching `RafEntry` (which may
/// currently live in `drain_due`'s local `taken` Vec rather than
/// in `MAIN_QUEUE.rafs`) is skipped on the next tick and dropped
/// during the merge. Worker pulses are gated on `RAF_COUNT`
/// reaching zero, not on `MAIN_QUEUE.rafs` being empty — the
/// latter is unreliable mid-drain.
struct RafHandle {
    id: u64,
    alive: Rc<Cell<bool>>,
}

impl ScheduleHandle for RafHandle {
    fn cancel(&mut self) {
        // Idempotent: subsequent `cancel` calls (cancel-then-drop,
        // double drop) shouldn't decrement the count twice.
        if !self.alive.get() {
            return;
        }
        self.alive.set(false);
        let prev = RAF_COUNT.fetch_sub(1, Ordering::SeqCst);
        if prev == 1 {
            if let Some(tx) = CMD_TX.lock().unwrap().clone() {
                let _ = tx.send(WorkerCmd::DisableRaf);
            }
        }
        // Best-effort eager cleanup. If we're mid-`drain_due`, the
        // entry is in `taken` not `MAIN_QUEUE.rafs`; the retain is a
        // no-op then, but the merge step below filters by `alive`
        // so the entry won't come back. `try_with` for the same
        // thread-teardown reason as `TimerHandle::cancel` above.
        let id = self.id;
        let _ = MAIN_QUEUE.try_with(|q| {
            q.borrow_mut().rafs.retain(|e| e.id != id);
        });
    }
}

impl Drop for RafHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

// ===========================================================================
// Tests — the new-core post-dispatch hook contract on this scheduler.
//
// Host-testable without a winit event loop: `after_ms_raw` and the
// `Scheduler` impl only touch `MAIN_QUEUE` (thread-local) when `CMD_TX`
// is empty (no worker running), and `drain_due` fires due closures on
// the calling thread — exactly what the `user_event` handler does. The
// hook slot lives in `render_wgpu::dispatch_hook` (also thread-local),
// so each test thread is isolated.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use render_wgpu::dispatch_hook::{clear_dispatch_hook, install_dispatch_hook};

    thread_local! {
        /// Ordered event log: "cb" entries from scheduled closures,
        /// "hook" entries from the dispatch hook. Thread-local so
        /// parallel tests don't interleave.
        static LOG: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    fn log_hook() {
        LOG.with(|l| l.borrow_mut().push("hook"));
    }

    fn take_log() -> Vec<&'static str> {
        LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
    }

    /// `after_ms` timers run author code → the hook must fire AFTER the
    /// callback returns (the flush-driver route for timer-staged
    /// writes).
    #[test]
    fn after_ms_fires_hook_after_callback() {
        install_dispatch_hook(log_hook);
        let handle = WinitScheduler.after_ms(0, Box::new(|| {
            LOG.with(|l| l.borrow_mut().push("cb"));
        }));
        assert!(take_log().is_empty(), "nothing fires before the drain");
        drain_due();
        assert_eq!(take_log(), vec!["cb", "hook"], "hook fires after the author callback");
        drop(handle); // spent timer; cancel is a no-op
        clear_dispatch_hook();
    }

    /// `schedule_microtask` must NOT fire the hook: the new-core flush
    /// itself is dispatched as a microtask (a 0 ms timer here), so a
    /// hooked microtask would re-schedule a flush from inside the
    /// flush's own dispatch and spin the timer queue forever. This is
    /// the regression test for that re-arm loop.
    #[test]
    fn schedule_microtask_does_not_fire_hook() {
        install_dispatch_hook(log_hook);
        WinitScheduler.schedule_microtask(Box::new(|| {
            LOG.with(|l| l.borrow_mut().push("cb"));
        }));
        drain_due();
        assert_eq!(
            take_log(),
            vec!["cb"],
            "microtasks are excluded from the post-dispatch hook"
        );
        clear_dispatch_hook();
    }

    /// `after_animation_frame` routes through `after_ms`, so frame
    /// one-shots (animation ticks that stage writes) get the hook too.
    #[test]
    fn after_animation_frame_fires_hook() {
        install_dispatch_hook(log_hook);
        let handle = WinitScheduler.after_animation_frame(Box::new(|| {
            LOG.with(|l| l.borrow_mut().push("cb"));
        }));
        // The 16 ms deadline is in the future; force it due by waiting.
        std::thread::sleep(Duration::from_millis(20));
        drain_due();
        assert_eq!(take_log(), vec!["cb", "hook"]);
        drop(handle);
        clear_dispatch_hook();
    }

    /// Every `raf_loop` iteration fires the hook (per-frame author code
    /// staging writes commits once per drained frame), and cancelling
    /// the loop stops both the callback and the hook.
    #[test]
    fn raf_loop_fires_hook_per_iteration_until_cancelled() {
        install_dispatch_hook(log_hook);
        let mut handle = WinitScheduler.raf_loop(Box::new(|| {
            LOG.with(|l| l.borrow_mut().push("cb"));
        }));
        drain_due();
        drain_due();
        assert_eq!(take_log(), vec!["cb", "hook", "cb", "hook"]);
        handle.cancel();
        drain_due();
        assert!(take_log().is_empty(), "cancelled loop is silent");
        drop(handle);
        clear_dispatch_hook();
    }

    /// With no hook installed (old core), the wrapped closures degrade
    /// to plain dispatch — one thread-local read, no behavior change.
    #[test]
    fn no_hook_installed_is_plain_dispatch() {
        clear_dispatch_hook();
        let handle = WinitScheduler.after_ms(0, Box::new(|| {
            LOG.with(|l| l.borrow_mut().push("cb"));
        }));
        drain_due();
        assert_eq!(take_log(), vec!["cb"]);
        drop(handle);
    }

    /// Regression: a `ScheduleHandle` dropped during THREAD TEARDOWN —
    /// after `MAIN_QUEUE`'s TLS destructor already ran — must not abort.
    /// This is exactly what `std::process::exit` triggers on macOS: the
    /// main thread's TLS destructors run, `runtime_core::scheduling::
    /// DETACHED_TASKS` drops its parked timer handles, and (before the
    /// `try_with` fix in `TimerHandle::cancel`) the cancel's
    /// `MAIN_QUEUE.with` panicked inside a destructor → non-unwinding
    /// abort of the whole exit (hit live by newcore-gpu-smoke's
    /// self-test exit).
    ///
    /// Reproduction relies on macOS/_tlv_atexit running TLS destructors
    /// in reverse registration order: HOLDER is touched FIRST (destroyed
    /// last), MAIN_QUEUE initializes second (destroyed first), so
    /// HOLDER's drop → `cancel` runs against a destroyed MAIN_QUEUE. On
    /// platforms with a different dtor order the test degrades to a
    /// benign drop — never to a false failure. Before the fix this test
    /// aborts the test process; after it, the thread exits cleanly.
    #[test]
    fn handle_drop_after_tls_teardown_does_not_abort() {
        thread::Builder::new()
            .name("tls-teardown-repro".into())
            .spawn(|| {
                thread_local! {
                    static HOLDER: RefCell<Vec<Box<dyn ScheduleHandle>>> =
                        const { RefCell::new(Vec::new()) };
                }
                // Register HOLDER's destructor before MAIN_QUEUE exists
                // on this thread.
                HOLDER.with(|_| {});
                // Registers MAIN_QUEUE (timer insert) — its dtor lands
                // after HOLDER's, so it runs first at thread exit.
                let timer = WinitScheduler.after_ms(60_000, Box::new(|| {}));
                let raf = WinitScheduler.raf_loop(Box::new(|| {}));
                HOLDER.with(|h| {
                    h.borrow_mut().push(timer);
                    h.borrow_mut().push(raf);
                });
                // Thread exits holding live handles; drops fire from
                // HOLDER's TLS destructor.
            })
            .expect("spawn repro thread")
            .join()
            .expect("thread exited cleanly (no destructor abort)");
    }
}
