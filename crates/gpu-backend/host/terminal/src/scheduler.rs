//! Single-threaded `Scheduler` implementation for the terminal host.
//!
//! The framework's animation system, presence-anim machinery, and
//! `after_ms` / `raf_loop` helpers all dispatch through
//! [`runtime_core::scheduling::Scheduler`]. The trait requires
//! `Send + Sync` because the global registry is `OnceLock`-backed —
//! but our terminal host is single-threaded, so the trait object is
//! a zero-sized type and every operation goes through thread-locals.
//!
//! The actual queue state lives in [`TICK_STATE`]; the host's
//! per-frame [`tick`] call drains expired `after_ms` callbacks,
//! pumps `raf_loop` subscribers, and flushes microtasks. If anything
//! is queued, [`has_pending`] returns true so the host can skip the
//! idle sleep and keep redrawing — that's what lets animations
//! actually animate.

use std::cell::RefCell;
use std::collections::BinaryHeap;
use std::time::{Duration, Instant};

use runtime_core::scheduling::{ScheduleHandle, Scheduler};

// ---------------------------------------------------------------------------
// Queue state — thread-local, accessed only from the host thread.
// ---------------------------------------------------------------------------

thread_local! {
    static TICK_STATE: RefCell<TickState> = RefCell::new(TickState::default());
}

#[derive(Default)]
struct TickState {
    next_id: u64,
    microtasks: Vec<Box<dyn FnOnce() + 'static>>,
    /// `after_ms` callbacks ordered by deadline (min-heap via
    /// `Reverse(Instant)`). Each entry carries the stable id the
    /// cancel handle uses to skip already-cancelled work.
    timers: BinaryHeap<TimerEntry>,
    /// `after_animation_frame` callbacks; fired once on the next
    /// `tick(...)` call.
    next_frame: Vec<NextFrameEntry>,
    /// Recurring `raf_loop` subscribers.
    raf: Vec<RafEntry>,
    /// Ids whose handles have been dropped — the matching entry is
    /// skipped when popped / iterated.
    cancelled: std::collections::HashSet<u64>,
}

struct TimerEntry {
    deadline: Instant,
    id: u64,
    f: Box<dyn FnOnce() + 'static>,
}

impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline
    }
}
impl Eq for TimerEntry {}
impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse so BinaryHeap behaves as a min-heap on deadline.
        other.deadline.cmp(&self.deadline)
    }
}

struct NextFrameEntry {
    id: u64,
    f: Box<dyn FnOnce() + 'static>,
}

struct RafEntry {
    id: u64,
    f: Box<dyn FnMut() + 'static>,
}

fn alloc_id() -> u64 {
    TICK_STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.next_id += 1;
        s.next_id
    })
}

// ---------------------------------------------------------------------------
// Scheduler trait impl. The struct is a ZST so `Send + Sync` is
// vacuously satisfied — every method routes through `TICK_STATE`.
// ---------------------------------------------------------------------------

pub(crate) struct TerminalScheduler;

impl Scheduler for TerminalScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        TICK_STATE.with(|s| s.borrow_mut().microtasks.push(f));
    }

    fn after_animation_frame(
        &self,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        let id = alloc_id();
        TICK_STATE.with(|s| {
            s.borrow_mut().next_frame.push(NextFrameEntry { id, f });
        });
        Box::new(TerminalHandle { id })
    }

    fn after_ms(
        &self,
        delay_ms: i32,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        let id = alloc_id();
        let delay = Duration::from_millis(delay_ms.max(0) as u64);
        let deadline = Instant::now() + delay;
        TICK_STATE.with(|s| {
            s.borrow_mut().timers.push(TimerEntry { deadline, id, f });
        });
        Box::new(TerminalHandle { id })
    }

    fn raf_loop(&self, f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        let id = alloc_id();
        TICK_STATE.with(|s| s.borrow_mut().raf.push(RafEntry { id, f }));
        Box::new(TerminalHandle { id })
    }
}

// The scheduler is a ZST with all state in thread-locals; the
// `Send + Sync` requirement of the framework's trait is therefore
// safe — no inner data crosses threads.
unsafe impl Send for TerminalScheduler {}
unsafe impl Sync for TerminalScheduler {}

struct TerminalHandle {
    id: u64,
}

impl ScheduleHandle for TerminalHandle {
    fn cancel(&mut self) {
        // Cheap: tag the id as cancelled, skip on drain. We don't
        // try to remove from the heap — heap removal would be O(n)
        // and tombstoning is fine since drains happen every frame.
        //
        // `try_with` + `try_borrow_mut`, NOT a plain `TICK_STATE.with` /
        // `borrow_mut`, because `cancel()` runs from `Drop`. These handles
        // are owned by the reactive runtime's scopes; at process teardown
        // those scopes drop and cancel their handles — sometimes AFTER this
        // thread-local has already been destroyed. A plain `with` then
        // panics inside a destructor ("cannot access a TLS value during or
        // after destruction"), which std escalates to a hard process abort
        // ("thread local panicked on drop, aborting"). Local-mount terminal
        // apps hit this on exit because they own the reactive graph
        // in-process; the runtime-server client never did (it only renders
        // streamed wire commands, so it holds no schedule handles) — which
        // is why `--runtime-server` worked while local crashed. If the TLS
        // is gone (or transiently borrowed), there's nothing left to cancel,
        // so drop the request silently. Regression:
        // `cancel_is_reentrancy_safe_when_tick_state_unavailable`.
        let _ = TICK_STATE.try_with(|s| {
            if let Ok(mut state) = s.try_borrow_mut() {
                state.cancelled.insert(self.id);
            }
        });
    }
}

impl Drop for TerminalHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

// ---------------------------------------------------------------------------
// Host-side hooks
// ---------------------------------------------------------------------------

/// Drain everything the host owes the framework before painting:
/// microtasks → expired timers → next-frame one-shots → recurring
/// `raf_loop` subscribers. Called once per frame by the host's
/// render loop.
pub(crate) fn tick() {
    let now = Instant::now();

    // 1) Microtasks. Drain into a local first so callbacks that
    //    enqueue more microtasks (the common pattern) get them on
    //    THIS tick, not next — same posture as a JS microtask
    //    queue.
    loop {
        let drained: Vec<_> =
            TICK_STATE.with(|s| std::mem::take(&mut s.borrow_mut().microtasks));
        if drained.is_empty() {
            break;
        }
        for f in drained {
            f();
        }
    }

    // 2) Expired timers. Pop while the top of the heap is due.
    loop {
        let entry = TICK_STATE.with(|s| {
            let mut s = s.borrow_mut();
            let due = s
                .timers
                .peek()
                .map(|t| t.deadline <= now)
                .unwrap_or(false);
            if !due {
                return None;
            }
            s.timers.pop()
        });
        let Some(entry) = entry else { break };
        let cancelled =
            TICK_STATE.with(|s| s.borrow_mut().cancelled.remove(&entry.id));
        if !cancelled {
            (entry.f)();
        }
    }

    // 3) Next-frame one-shots. Drain wholesale.
    let next_frame: Vec<_> =
        TICK_STATE.with(|s| std::mem::take(&mut s.borrow_mut().next_frame));
    for entry in next_frame {
        let cancelled =
            TICK_STATE.with(|s| s.borrow_mut().cancelled.remove(&entry.id));
        if !cancelled {
            (entry.f)();
        }
    }

    // 4) `raf_loop` subscribers. We swap the vec out, fire each
    //    closure, then put it back — so callbacks dropping their
    //    own handle mid-loop don't perturb iteration. Cancelled
    //    handles are filtered after the swap-back.
    let mut raf: Vec<RafEntry> =
        TICK_STATE.with(|s| std::mem::take(&mut s.borrow_mut().raf));
    for entry in raf.iter_mut() {
        let cancelled =
            TICK_STATE.with(|s| s.borrow().cancelled.contains(&entry.id));
        if !cancelled {
            (entry.f)();
        }
    }
    // Drop cancelled entries and merge back any newly-registered
    // ones from inside callbacks (cancelled set is consulted again
    // on next tick).
    TICK_STATE.with(|s| {
        let mut state = s.borrow_mut();
        let mut kept: Vec<RafEntry> = Vec::with_capacity(raf.len());
        for entry in raf {
            if !state.cancelled.contains(&entry.id) {
                kept.push(entry);
            }
        }
        // Append any raf subscribers registered during this tick.
        kept.extend(std::mem::take(&mut state.raf));
        state.raf = kept;
        // Garbage-collect cancelled tombstones for ids we've now
        // forgotten. The set shouldn't grow unboundedly — pruning
        // every tick keeps it bounded by "things cancelled in the
        // last frame".
        state.cancelled.clear();
    });
}

/// `true` if any timer is due soon, a raf subscriber is active, or
/// microtasks are queued. Lets the host skip its idle sleep so the
/// next frame paints animations instead of stalling on `poll`.
pub(crate) fn has_pending() -> bool {
    TICK_STATE.with(|s| {
        let s = s.borrow();
        !s.microtasks.is_empty()
            || !s.next_frame.is_empty()
            || !s.raf.is_empty()
            || !s.timers.is_empty()
    })
}

/// Install the scheduler on this thread. Idempotent — only the
/// first call wins (per the framework's `OnceLock` contract).
pub fn install() {
    runtime_core::scheduling::install_scheduler(Box::new(TerminalScheduler));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: dropping/cancelling a `TerminalHandle` must NOT panic
    /// when `TICK_STATE` can't be freely accessed.
    ///
    /// The production crash was a local-mount terminal app aborting on exit
    /// with "thread local panicked on drop": the reactive runtime's scopes
    /// drop their schedule handles at teardown, and `TerminalHandle::Drop ->
    /// cancel()` re-entered `TICK_STATE` after that thread-local had already
    /// been destroyed — a panic inside a destructor, which std escalates to
    /// a hard abort. The true teardown ordering (TLS destroyed) is only
    /// reachable at thread exit, not in a unit test, so we exercise the
    /// closest reachable form of the same hazard: cancelling while
    /// `TICK_STATE` is already borrowed. The `try_borrow_mut` guard makes
    /// that a silent no-op; before the fix the unconditional `borrow_mut`
    /// double-borrow-panicked here exactly as it aborted at teardown.
    #[test]
    fn cancel_is_reentrancy_safe_when_tick_state_unavailable() {
        TICK_STATE.with(|s| {
            // Hold a shared borrow, then drop a handle: its `cancel()` wants
            // `borrow_mut()` and must back off instead of panicking.
            let _guard = s.borrow();
            let handle = TerminalHandle { id: 42 };
            drop(handle); // must not panic
        });

        // And the normal (uncontended) path still records the cancellation.
        let mut handle = TerminalHandle { id: 7 };
        handle.cancel();
        let recorded = TICK_STATE.with(|s| s.borrow().cancelled.contains(&7));
        assert!(recorded, "uncontended cancel should still tag the id");
    }
}
