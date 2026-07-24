//! Message-loop-integrated [`Scheduler`] for the Win32 host.
//!
//! `runtime_core`'s scheduling helpers (`after_ms`, `raf_loop`,
//! `schedule_microtask`) all dispatch through one installed
//! [`Scheduler`]. Without a real one, `after_ms` runs synchronously
//! (delay ignored — every act of a timeline collapses to one instant)
//! and `raf_loop` is inert (so `AnimatedValue`s never tick). The Win32
//! host must install this before the app mounts.
//!
//! # Design
//!
//! This is a direct port of the winit host's scheduler
//! (`host-winit/src/scheduler.rs`) — the design is identical, only the
//! main-thread *wake* differs:
//!
//! - **Closures live on the main thread** (`MAIN_QUEUE` thread-local).
//!   The `Send + Sync` bound on `Scheduler` would otherwise force the
//!   `FnOnce`/`FnMut` closures themselves to be `Send`, which the
//!   framework's `Rc`-capturing builders are not.
//! - **A single worker thread** holds the deadlines and sleeps until
//!   the next one. On fire it wakes the main thread — here by
//!   `PostMessageW`-ing `WM_IDEALYST_SCHED` to the host window (winit
//!   sends an `EventLoopProxy` event). `PostMessageW` is thread-safe
//!   and the canonical cross-thread wake for a Win32 message pump.
//! - The window's `WndProc` handles `WM_IDEALYST_SCHED` by calling
//!   [`drain_due`], which runs every callback whose deadline has
//!   passed plus one tick of each live `raf_loop` client.
//!
//! Cancellation is cooperative: a handle's `Drop` removes the entry
//! from `MAIN_QUEUE`; the worker discovers the absence on its next
//! wake and the main-thread drain step skips it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicIsize, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use runtime_core::driver::{
    install_render_loop_driver, RenderLoopDriver, RenderLoopHandle,
};
use runtime_core::scheduling::{install_scheduler, raf_loop, RafLoop, ScheduleHandle, Scheduler};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::app::WM_IDEALYST_SCHED;

/// Commands the main thread sends to the worker.
enum WorkerCmd {
    AfterMs { id: u64, deadline: Instant },
    EnableRaf,
    DisableRaf,
}

/// One pending one-shot timer. The deadline is duplicated (the worker
/// also tracks it) so the drain step fires only what's actually due.
struct PendingTimer {
    f: Option<Box<dyn FnOnce() + 'static>>,
    deadline: Instant,
}

/// One active raf-loop client. `alive` is shared with the matching
/// [`RafHandle`] so a cancel that fires mid-tick (while `drain_due`
/// has `mem::take`-n the Vec) is still honored on the merge back.
struct RafEntry {
    id: u64,
    f: Box<dyn FnMut() + 'static>,
    alive: Rc<Cell<bool>>,
}

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

/// Worker's command sender, set once at [`install`].
static CMD_TX: Mutex<Option<Sender<WorkerCmd>>> = Mutex::new(None);

/// HWND (as `isize`) the worker `PostMessageW`s to wake the main
/// thread. Set once at [`install`]; read by the worker on every fire.
static WAKE_HWND: AtomicIsize = AtomicIsize::new(0);

/// Monotonic id allocator for both timer and raf entries.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Live raf-client count. The worker pulses at 60 Hz while this is
/// `> 0`. Tracked separately from `MAIN_QUEUE.rafs.len()` so a
/// `RafHandle::drop` firing mid-`drain_due` (when the Vec is taken
/// into a local) can't observe an empty Vec and spuriously stop
/// pulses for other live clients.
static RAF_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Post the wake message to the host window. Thread-safe; called from
/// the worker thread. A `false` return means the window is gone (app
/// tearing down) — harmless, the worker exits when the channel drops.
fn post_wake() {
    let raw = WAKE_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return;
    }
    let hwnd = HWND(raw as *mut c_void);
    unsafe {
        let _ = PostMessageW(hwnd, WM_IDEALYST_SCHED, WPARAM(0), LPARAM(0));
    }
}

/// Install the Win32 host scheduler. Called once from `run_with`
/// BEFORE the app mounts. `hwnd` is the host window the worker wakes.
/// Idempotent: a second call (only possible if a process runs two
/// hosts) short-circuits once the worker is up.
pub(crate) fn install(hwnd: HWND) {
    WAKE_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
    {
        let mut slot = CMD_TX.lock().unwrap();
        if slot.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel::<WorkerCmd>();
        *slot = Some(tx);
        thread::Builder::new()
            .name("idealyst-win32-scheduler".to_string())
            .spawn(move || worker_main(rx))
            .expect("spawn scheduler worker");
    }
    install_scheduler(Box::new(Win32Scheduler));
    // Embedded wgpu hosts (host-windows-desktop behind the website's
    // Simulator) tick via `runtime_core::driver::render_loop`; without
    // an installed driver the returned handle is inert and the canvas
    // never presents a frame. Ride the same 60 Hz raf pulse the
    // scheduler already delivers.
    install_render_loop_driver(Box::new(Win32RenderLoopDriver));
}

/// [`RenderLoopDriver`] on top of [`raf_loop`] — one `elapsed_seconds`
/// clock per started loop, ticked by the scheduler's 60 Hz pulse on
/// the UI thread (the same wake that drives `AnimatedValue`s).
struct Win32RenderLoopDriver;

struct Win32RenderLoopHandle(RafLoop);

impl RenderLoopHandle for Win32RenderLoopHandle {
    fn cancel(&mut self) {
        self.0.cancel();
    }
}

impl RenderLoopDriver for Win32RenderLoopDriver {
    fn start(&self, mut closure: Box<dyn FnMut(f32) + 'static>) -> Box<dyn RenderLoopHandle> {
        let start = Instant::now();
        Box::new(Win32RenderLoopHandle(raf_loop(move || {
            closure(start.elapsed().as_secs_f32());
        })))
    }
}

#[cfg(test)]
mod tests {
    //! Drop-cascade regressions: cancelling a queue entry whose closure
    //! OWNS other schedule handles must not drop it while the queue is
    //! borrowed — the inner handles' `cancel` re-enters
    //! `MAIN_QUEUE.borrow_mut()`. Real-world chain: the website
    //! Simulator's `on_lost` drops its wgpu `WindowsHostHandle` →
    //! `RenderLoop` raf cancel → dropping the raf closure drops
    //! `HostInner` → its session scope drops scope-anchored timers →
    //! "RefCell already borrowed" panic, crashing the app on every
    //! navigation away from the home page.
    //!
    //! No worker thread is running in tests (`CMD_TX` is `None`), so
    //! entries just sit in the thread-local queue — exactly what the
    //! cascade needs.

    use super::*;

    #[test]
    fn regression_timer_cancel_drop_cascade_does_not_reenter_queue() {
        let sched = Win32Scheduler;
        let inner = sched.after_ms(600_000, Box::new(|| {}));
        // Outer pending timer whose CLOSURE owns the inner handle.
        let mut outer = sched.after_ms(
            600_000,
            Box::new(move || {
                let _keep = &inner;
            }),
        );
        // Pre-fix: cancel removed the entry and dropped its closure
        // (and thus `inner`) inside the queue borrow → panic.
        outer.cancel();
    }

    #[test]
    fn regression_raf_cancel_drop_cascade_does_not_reenter_queue() {
        let sched = Win32Scheduler;
        let timer = sched.after_ms(600_000, Box::new(|| {}));
        let mut raf = sched.raf_loop(Box::new(move || {
            let _keep = &timer;
        }));
        raf.cancel();
    }

    #[test]
    fn regression_drain_drops_mid_tick_cancelled_raf_outside_borrow() {
        let sched = Win32Scheduler;
        // Raf B's closure owns a pending timer handle.
        let timer = sched.after_ms(600_000, Box::new(|| {}));
        let raf_b = Rc::new(RefCell::new(Some(sched.raf_loop(Box::new(move || {
            let _keep = &timer;
        })))));
        // Raf A cancels B mid-tick: B is in the drained `taken` Vec at
        // that point (its own cancel finds `q.rafs` empty), so the
        // DRAIN's merge step is what drops B's closure. Pre-fix that
        // drop ran inside the merge borrow → panic.
        let raf_b_for_a = raf_b.clone();
        let _raf_a = sched.raf_loop(Box::new(move || {
            if let Some(mut b) = raf_b_for_a.borrow_mut().take() {
                b.cancel();
            }
        }));
        drain_due();
    }
}

/// Worker thread. Keeps a sorted deadline list + a raf-active flag,
/// sleeps until the nearest wake, then posts `WM_IDEALYST_SCHED`. It
/// never holds the closures — those live on the main thread; a missed
/// deadline (closure cancelled before fire) makes the drain a no-op.
fn worker_main(rx: mpsc::Receiver<WorkerCmd>) {
    const RAF_PERIOD: Duration = Duration::from_millis(16);

    let mut timers: Vec<(Instant, u64)> = Vec::new();
    let mut raf_active = false;
    let mut next_raf = Instant::now();

    loop {
        loop {
            match rx.try_recv() {
                Ok(cmd) => apply(cmd, &mut timers, &mut raf_active, &mut next_raf),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        let now = Instant::now();

        let mut fired_any = false;
        while let Some(&(deadline, _)) = timers.first() {
            if deadline > now {
                break;
            }
            timers.remove(0);
            fired_any = true;
        }
        if fired_any {
            post_wake();
        }

        if raf_active && now >= next_raf {
            post_wake();
            next_raf = now + RAF_PERIOD;
        }

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
                        Ok(cmd) => apply(cmd, &mut timers, &mut raf_active, &mut next_raf),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
            }
            None => match rx.recv() {
                Ok(cmd) => apply(cmd, &mut timers, &mut raf_active, &mut next_raf),
                Err(_) => return,
            },
        }
    }
}

fn apply(
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

/// Drain every due timer + tick every live raf client. Called from the
/// host `WndProc` on `WM_IDEALYST_SCHED`.
pub(crate) fn drain_due() {
    let now = Instant::now();
    // Move due `FnOnce`s out under a short borrow so a callback that
    // re-enters `after_ms` doesn't trip the RefCell.
    let to_fire: Vec<Box<dyn FnOnce() + 'static>> = MAIN_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
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

    // Tick raf clients. Swap the Vec out so a closure can register new
    // rafs mid-tick; skip + drop entries cancelled mid-tick (reported
    // via `alive`), otherwise a re-registration would resurrect a
    // just-cancelled entry.
    let mut taken: Vec<RafEntry> = MAIN_QUEUE.with(|q| std::mem::take(&mut q.borrow_mut().rafs));
    for entry in taken.iter_mut() {
        if entry.alive.get() {
            (entry.f)();
        }
    }
    // Drop mid-tick-cancelled entries BEFORE re-borrowing the queue:
    // a dead raf closure's captures can cascade into further handle
    // drops (render-loop closure → wgpu HostInner → session scope →
    // scope-anchored timers), each re-entering `MAIN_QUEUE` — dropping
    // them under the merge borrow is the "RefCell already borrowed"
    // crash the Simulator unmount hit.
    taken.retain(|e| e.alive.get());
    MAIN_QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let mut merged = taken;
        merged.append(&mut q.rafs);
        q.rafs = merged;
    });
}

/// Zero-sized scheduler stored in the framework's slot; all live state
/// is in `MAIN_QUEUE` (per-thread closures) + `CMD_TX` (worker channel).
struct Win32Scheduler;

// SAFETY: the struct holds no state — `CMD_TX`/`WAKE_HWND` are globals
// and the closures live in thread-local storage, so the `Send + Sync`
// bound is satisfied by the empty struct alone. Mirrors the winit /
// iOS scheduler impls.
unsafe impl Send for Win32Scheduler {}
unsafe impl Sync for Win32Scheduler {}

impl Scheduler for Win32Scheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        // Run after the current synchronous stack unwinds: a 0 ms timer
        // lands it in the next message-loop iteration. Forget the
        // handle — microtasks are fire-and-forget; dropping it would
        // cancel the timer the framework just scheduled.
        std::mem::forget(self.after_ms(0, f));
    }

    fn after_animation_frame(&self, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        self.after_ms(16, f)
    }

    fn after_ms(&self, delay_ms: i32, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_millis(delay_ms.max(0) as u64);
        MAIN_QUEUE.with(|q| {
            q.borrow_mut()
                .timers
                .insert(id, PendingTimer { f: Some(f), deadline });
        });
        if let Some(tx) = CMD_TX.lock().unwrap().clone() {
            let _ = tx.send(WorkerCmd::AfterMs { id, deadline });
        }
        Box::new(TimerHandle { id })
    }

    fn raf_loop(&self, f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let alive = Rc::new(Cell::new(true));
        MAIN_QUEUE.with(|q| {
            q.borrow_mut().rafs.push(RafEntry {
                id,
                f,
                alive: alive.clone(),
            });
        });
        let prev = RAF_COUNT.fetch_add(1, Ordering::SeqCst);
        if prev == 0 {
            if let Some(tx) = CMD_TX.lock().unwrap().clone() {
                let _ = tx.send(WorkerCmd::EnableRaf);
            }
        }
        Box::new(RafHandle { id, alive })
    }
}

struct TimerHandle {
    id: u64,
}

impl ScheduleHandle for TimerHandle {
    fn cancel(&mut self) {
        // `try_with`, not `with`: a handle owned by a reactive effect
        // can be dropped during end-of-thread TLS teardown, where
        // `MAIN_QUEUE` may already be destroyed. Removing an entry from
        // a queue that's being torn down is moot, so a failed access is
        // a no-op rather than a panic. (The host normally exits via
        // `TerminateProcess`, skipping teardown entirely — this keeps
        // the handle correct even if a drop path ever runs under it.)
        //
        // The removed entry is dropped AFTER the borrow ends: its
        // pending closure's captures can cascade into MORE handle
        // drops (a render-loop closure owns a wgpu HostInner whose
        // session scope anchors timers), and each of those re-enters
        // `MAIN_QUEUE.borrow_mut()` — a drop inside this borrow is the
        // "RefCell already borrowed" crash the Simulator unmount hit.
        let removed = MAIN_QUEUE.try_with(|q| q.borrow_mut().timers.remove(&self.id));
        drop(removed);
    }
}

impl Drop for TimerHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct RafHandle {
    id: u64,
    alive: Rc<Cell<bool>>,
}

impl ScheduleHandle for RafHandle {
    fn cancel(&mut self) {
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
        let id = self.id;
        // `try_with` for the same teardown-safety reason as
        // `TimerHandle::cancel` — and, like there, the removed entry
        // must drop AFTER the borrow: the raf closure's captures can
        // cascade into further handle drops that re-enter the queue.
        let removed = MAIN_QUEUE.try_with(|q| {
            let mut queue = q.borrow_mut();
            let mut out: Vec<RafEntry> = Vec::new();
            let mut i = 0;
            while i < queue.rafs.len() {
                if queue.rafs[i].id == id {
                    out.push(queue.rafs.remove(i));
                } else {
                    i += 1;
                }
            }
            out
        });
        drop(removed);
    }
}

impl Drop for RafHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}
