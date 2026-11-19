//! `ScheduledTask` with a real (queueing) scheduler installed: dropping the
//! handle cancels the callback, and `detach()` lets it fire with nothing
//! holding it.
//!
//! Its own test binary because the scheduler is a process-global
//! `OnceLock`: the unit tests in `scheduling.rs` cover the no-scheduler
//! fallback and must not see one installed.
#![cfg(not(target_arch = "wasm32"))]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use runtime_shared::scheduling::{
    after_animation_frame, after_ms, after_ms_detached, install_scheduler, ScheduleHandle, Scheduler,
};

type Slot = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

thread_local! {
    static QUEUE: RefCell<Vec<Slot>> = const { RefCell::new(Vec::new()) };
}

struct Handle(Slot);

impl ScheduleHandle for Handle {
    fn cancel(&mut self) {
        self.0.borrow_mut().take();
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Queueing;

impl Queueing {
    fn queue(f: Box<dyn FnOnce()>) -> Box<dyn ScheduleHandle> {
        let slot: Slot = Rc::new(RefCell::new(Some(f)));
        QUEUE.with(|q| q.borrow_mut().push(slot.clone()));
        Box::new(Handle(slot))
    }
}

impl Scheduler for Queueing {
    fn schedule_microtask(&self, f: Box<dyn FnOnce()>) {
        f();
    }
    fn after_animation_frame(&self, f: Box<dyn FnOnce()>) -> Box<dyn ScheduleHandle> {
        Queueing::queue(f)
    }
    fn after_ms(&self, _delay_ms: i32, f: Box<dyn FnOnce()>) -> Box<dyn ScheduleHandle> {
        Queueing::queue(f)
    }
    fn raf_loop(&self, _f: Box<dyn FnMut()>) -> Box<dyn ScheduleHandle> {
        Box::new(Handle(Rc::new(RefCell::new(None))))
    }
}

/// Fire everything queued on this thread (cancelled slots are empty).
fn fire() {
    let slots = QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()));
    for slot in slots {
        let f = slot.borrow_mut().take();
        if let Some(f) = f {
            f();
        }
    }
}

fn counter() -> (Rc<Cell<u32>>, impl FnOnce() + 'static) {
    let n = Rc::new(Cell::new(0));
    let m = n.clone();
    (n, move || m.set(m.get() + 1))
}

/// Timely: `after_animation_frame(..)` as a fire-and-forget statement never
/// ran — the returned handle dropped at the semicolon and cancelled it.
/// The handle is `#[must_use]` now (a compile-time warning, not testable
/// here); this pins the drop-cancels behaviour that makes it necessary and
/// the `detach()` that replaces the statement.
#[test]
fn regression_a_dropped_frame_handle_cancels_and_a_detached_one_fires() {
    install_scheduler(Box::new(Queueing));

    let (dropped, f) = counter();
    drop(after_animation_frame(f));
    fire();
    assert_eq!(dropped.get(), 0, "a dropped handle cancels its callback");

    let (detached, f) = counter();
    after_animation_frame(f).detach();
    fire();
    assert_eq!(detached.get(), 1, "a detached task fires with nothing holding it");

    let (held, f) = counter();
    let task = after_ms(10, f);
    fire();
    assert_eq!(held.get(), 1, "a held handle fires");
    drop(task);

    let (timer, f) = counter();
    after_ms_detached(10, f);
    fire();
    assert_eq!(timer.get(), 1, "after_ms_detached is after_ms(..).detach()");

    // Fired tasks are swept on the next detach; many in flight all fire.
    let (many, _) = counter();
    for _ in 0..50 {
        let m = many.clone();
        after_ms(0, move || m.set(m.get() + 1)).detach();
    }
    fire();
    assert_eq!(many.get(), 50);
}
