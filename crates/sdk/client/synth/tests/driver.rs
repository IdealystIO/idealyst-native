//! The `Mixer`'s real-time driver: with a scheduler installed it re-arms a
//! `PUMP_TICK_MS` timer chain that pumps every tick, and the chain ends once
//! every `Mixer` and stream handle is gone.
//!
//! Own test binary because `install_scheduler` is process-global (first
//! install wins). The fake scheduler queues `after_ms` bodies instead of
//! running them, so the test steps the chain one tick at a time.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use runtime_core::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use synth::{AudioFormat, Mixer, PUMP_TICK_MS};

type Task = (i32, Rc<Cell<bool>>, Box<dyn FnOnce()>);

thread_local! {
    static QUEUE: RefCell<Vec<Task>> = const { RefCell::new(Vec::new()) };
}

struct Handle(Rc<Cell<bool>>);
impl ScheduleHandle for Handle {
    fn cancel(&mut self) {
        self.0.set(true);
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

struct QueueScheduler;
impl Scheduler for QueueScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        f();
    }
    fn after_animation_frame(&self, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        panic!("the mixer must not use rAF (it stalls in hidden tabs)");
    }
    fn after_ms(&self, delay_ms: i32, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        let cancelled = Rc::new(Cell::new(false));
        QUEUE.with(|q| q.borrow_mut().push((delay_ms, cancelled.clone(), f)));
        Box::new(Handle(cancelled))
    }
    fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        panic!("the mixer must not use rAF (it stalls in hidden tabs)");
    }
}

/// Fire every queued, uncancelled timer once; return how many fired.
fn tick() -> usize {
    let due: Vec<Task> = QUEUE.with(|q| q.borrow_mut().drain(..).collect());
    let mut fired = 0;
    for (delay, cancelled, f) in due {
        assert_eq!(delay, PUMP_TICK_MS);
        if !cancelled.get() {
            f();
            fired += 1;
        }
    }
    fired
}

fn pending() -> usize {
    QUEUE.with(|q| q.borrow().iter().filter(|(_, c, _)| !c.get()).count())
}

#[test]
fn driver_pumps_every_tick_and_stops_when_all_handles_drop() {
    install_scheduler(Box::new(QueueScheduler));

    let mixer = Mixer::new(AudioFormat { sample_rate: 48_000, channels: 1 });
    let chunks = Arc::new(Mutex::new(0usize));
    let c = chunks.clone();
    let stream = mixer.stream();
    let _sub = stream.subscribe(move |_| *c.lock().unwrap() += 1);
    assert_eq!(pending(), 1, "new() arms the first tick");

    assert_eq!(tick(), 1);
    assert_eq!(*chunks.lock().unwrap(), 1, "first tick pumps (renders the lead)");
    assert_eq!(pending(), 1, "and re-arms");

    // Dropping the Mixer alone keeps it running: the stream is still held
    // (a recorder holding only the stream must keep getting audio).
    drop(mixer);
    assert_eq!(tick(), 1);
    assert_eq!(pending(), 1, "stream clone keeps the chain alive");

    // Last handle gone: the next tick finds the mixer dead and ends the chain.
    drop(stream);
    tick();
    assert_eq!(pending(), 0, "no timer left after the mixer is gone");
    tick();
    assert_eq!(pending(), 0);
}
