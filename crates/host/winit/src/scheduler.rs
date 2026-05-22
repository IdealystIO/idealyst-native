//! Native scheduler for the wgpu sim runtime.
//!
//! The framework's `framework_core::scheduling` helpers
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

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use framework_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
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

/// One active raf-loop client.
struct RafEntry {
    id: u64,
    f: Box<dyn FnMut() + 'static>,
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
    let mut taken: Vec<RafEntry> =
        MAIN_QUEUE.with(|q| std::mem::take(&mut q.borrow_mut().rafs));
    for entry in taken.iter_mut() {
        (entry.f)();
    }
    MAIN_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        // Merge: any rafs registered during the tick land in `q.rafs`;
        // splice the original survivors back at the front so order is
        // preserved.
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

impl Scheduler for WinitScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        // "Microtask" = run after the current synchronous stack
        // unwinds, on the same thread. Implementing it as a 0 ms
        // `after_ms` lands the closure in the next event-loop
        // iteration — same shape as iOS's NSTimer-based scheduler.
        let _ = self.after_ms(0, f);
    }

    fn after_animation_frame(
        &self,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        // Match the rest of the framework's scheduler impls — one
        // animation frame ≈ 16 ms. The worker may signal sooner
        // if a timer is due before the next raf pulse; either way
        // the closure fires once.
        self.after_ms(16, f)
    }

    fn after_ms(
        &self,
        delay_ms: i32,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
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

    fn raf_loop(&self, f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        MAIN_QUEUE.with(|q| {
            q.borrow_mut().rafs.push(RafEntry { id, f });
        });
        if let Some(tx) = CMD_TX.lock().unwrap().clone() {
            let _ = tx.send(WorkerCmd::EnableRaf);
        }
        Box::new(RafHandle { id })
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
        MAIN_QUEUE.with(|q| {
            q.borrow_mut().timers.remove(&self.id);
        });
    }
}

impl Drop for TimerHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Handle returned from `raf_loop`. `Drop` removes the entry from
/// the raf list; the worker stops pulsing once it sees the list is
/// empty (via the `DisableRaf` hint, sent below).
struct RafHandle {
    id: u64,
}

impl ScheduleHandle for RafHandle {
    fn cancel(&mut self) {
        let now_empty = MAIN_QUEUE.with(|q| {
            let mut q = q.borrow_mut();
            q.rafs.retain(|e| e.id != self.id);
            q.rafs.is_empty()
        });
        if now_empty {
            if let Some(tx) = CMD_TX.lock().unwrap().clone() {
                let _ = tx.send(WorkerCmd::DisableRaf);
            }
        }
    }
}

impl Drop for RafHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}
