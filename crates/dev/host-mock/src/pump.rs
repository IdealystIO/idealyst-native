//! Deterministic async for host-mock tests: a manually-pumped
//! [`AsyncExecutor`](runtime_shared::driver::AsyncExecutor) (lazy chunk
//! loads) and a manually-pumped
//! [`Scheduler`](runtime_shared::scheduling::Scheduler) (animation
//! frames + timers, for presence timing windows).
//!
//! Both registries are process-global `OnceLock`s (first install wins),
//! but every queue here is thread-local — each `#[test]` thread pumps
//! only its own tasks, so parallel tests can't cross-fire.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::Context;

use runtime_shared::scheduling::{install_scheduler as core_install_scheduler, ScheduleHandle, Scheduler};

// ===========================================================================
// Async executor (chunk loads)
// ===========================================================================

thread_local! {
    static TASKS: RefCell<Vec<Pin<Box<dyn Future<Output = ()>>>>> =
        const { RefCell::new(Vec::new()) };
}

struct QueueExec;

impl runtime_shared::driver::AsyncExecutor for QueueExec {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
        TASKS.with(|t| t.borrow_mut().push(future));
    }
}

/// Install the queueing executor (OnceLock global: first install wins;
/// every test thread shares the executor but queues into its OWN
/// thread-local task list).
pub fn install_executor() {
    runtime_shared::driver::install_async_executor(Box::new(QueueExec));
}

/// Poll every queued task once (noop waker — tests re-pump after
/// flipping their ready flags). Completed tasks drop; pending ones stay.
pub fn pump_tasks() {
    let mut tasks = TASKS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    let waker = std::task::Waker::noop();
    let mut cx = Context::from_waker(waker);
    tasks.retain_mut(|f| f.as_mut().poll(&mut cx).is_pending());
    TASKS.with(|t| t.borrow_mut().extend(tasks));
}

/// Number of still-pending spawned tasks on this thread.
pub fn pending_tasks() -> usize {
    TASKS.with(|t| t.borrow().len())
}

// ===========================================================================
// Scheduler (frames + timers)
// ===========================================================================

type Queued = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

thread_local! {
    static FRAME_QUEUE: RefCell<Vec<Queued>> = const { RefCell::new(Vec::new()) };
    static TIMER_QUEUE: RefCell<Vec<Queued>> = const { RefCell::new(Vec::new()) };
}

struct PumpHandle(Queued);

impl ScheduleHandle for PumpHandle {
    fn cancel(&mut self) {
        // Emptying the slot cancels: the pump skips vacated entries.
        self.0.borrow_mut().take();
    }
}

impl Drop for PumpHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct PumpScheduler;

impl Scheduler for PumpScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce()>) {
        // Tests never rely on microtask deferral; run inline.
        f();
    }

    fn after_animation_frame(&self, f: Box<dyn FnOnce()>) -> Box<dyn ScheduleHandle> {
        let slot: Queued = Rc::new(RefCell::new(Some(f)));
        FRAME_QUEUE.with(|q| q.borrow_mut().push(slot.clone()));
        Box::new(PumpHandle(slot))
    }

    fn after_ms(&self, _delay_ms: i32, f: Box<dyn FnOnce()>) -> Box<dyn ScheduleHandle> {
        let slot: Queued = Rc::new(RefCell::new(Some(f)));
        TIMER_QUEUE.with(|q| q.borrow_mut().push(slot.clone()));
        Box::new(PumpHandle(slot))
    }

    fn raf_loop(&self, _f: Box<dyn FnMut()>) -> Box<dyn ScheduleHandle> {
        let slot: Queued = Rc::new(RefCell::new(None));
        Box::new(PumpHandle(slot))
    }
}

/// Install the pumped scheduler (first call wins; later calls no-op).
pub fn install_scheduler() {
    core_install_scheduler(Box::new(PumpScheduler));
}

fn pump(queue: &'static std::thread::LocalKey<RefCell<Vec<Queued>>>) {
    let tasks: Vec<Queued> = queue.with(|q| std::mem::take(&mut *q.borrow_mut()));
    for slot in tasks {
        // Release the slot borrow BEFORE running: the callback may drop
        // its own spent ScheduledTask, whose cancel re-borrows the slot.
        let f = slot.borrow_mut().take();
        if let Some(f) = f {
            f();
        }
    }
}

/// Fire every queued animation-frame callback once.
pub fn pump_frames() {
    pump(&FRAME_QUEUE);
}

/// Fire every queued timer callback once (delays collapse).
pub fn pump_timers() {
    pump(&TIMER_QUEUE);
}
