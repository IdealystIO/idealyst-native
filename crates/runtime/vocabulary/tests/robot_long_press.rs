//! A long press through the robot's hit-test tier (a backend with no input
//! injection — iOS) fires the framework's own `LongPress` recognizer: the
//! held `press_native` arms the recognizer's `after_ms` timer, the timer
//! elapses while the press is held, and `release_native` lifts it.
//!
//! Own integration test (own process) because it installs a buffering test
//! scheduler through the global first-install-wins
//! `runtime_shared::scheduling::install_scheduler` slot; the timer queue is
//! pumped by hand where real time would pass.
#![cfg(feature = "robot")]

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use host_mock::Harness;
use runtime_scene::realize;
use runtime_shared::primitives::portal::ViewportRect;
use runtime_shared::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_shared::{long_press, LongPressRecognizer};
use runtime_vocabulary::builders::view;
use runtime_vocabulary::robot::{Query, Robot, TapMechanism};

thread_local! {
    static TIMERS: RefCell<VecDeque<Box<dyn FnOnce() + 'static>>> = RefCell::new(VecDeque::new());
}

struct TestScheduler;
struct InertHandle;
impl ScheduleHandle for InertHandle {
    fn cancel(&mut self) {}
}
// SAFETY: zero-sized; all live state is thread-local.
unsafe impl Send for TestScheduler {}
unsafe impl Sync for TestScheduler {}
impl Scheduler for TestScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        f();
    }
    fn after_animation_frame(&self, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        TIMERS.with(|q| q.borrow_mut().push_back(f));
        Box::new(InertHandle)
    }
    fn after_ms(&self, _delay_ms: i32, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        TIMERS.with(|q| q.borrow_mut().push_back(f));
        Box::new(InertHandle)
    }
    fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(InertHandle)
    }
    fn drain_buffered_microtasks(&self) {}
}

/// Run every queued timer: the hold's real time passing.
fn pump_timers() {
    while let Some(f) = TIMERS.with(|q| q.borrow_mut().pop_front()) {
        f();
    }
}

/// CrewForge E2E report B5: a touch platform's context gesture is a long
/// press, and on iOS (no input injection) `press_native` answered
/// "unsupported", so no test could hold one. Held through the hit test, the
/// recognizer fires while the press is down — not before, and not from a
/// tap.
#[test]
fn regression_long_press_through_the_hit_test_fires_the_recognizer() {
    install_scheduler(Box::new(TestScheduler));
    let robot = Robot::new();
    robot.reset();
    let h = Harness::new();
    let fired = Rc::new(Cell::new(0));
    let _r = {
        let fired = fired.clone();
        h.world.enter(|| {
            realize(
                &h.backend,
                &h.registry,
                view()
                    .child(
                        view()
                            .test_id("row")
                            .on_touch(long_press(LongPressRecognizer::new(), move || fired.set(fired.get() + 1)))
                            .build(),
                    )
                    .build(),
            )
        })
    };
    runtime_shared::viewport::set_viewport_size(runtime_shared::viewport::ViewportSize { width: 400.0, height: 300.0 });
    h.shared.frames.borrow_mut().insert(1, ViewportRect { x: 0.0, y: 0.0, width: 200.0, height: 50.0 });
    let row = robot.find(Query::test_id("row")).expect("row registered");

    // A tap: down and straight back up, the timer never sees a held press.
    assert_eq!(robot.tap_native(&row), Ok(TapMechanism::HitTest));
    pump_timers();
    assert_eq!(fired.get(), 0, "a tap is not a long press");

    assert_eq!(robot.press_native(&row), Ok(TapMechanism::HitTest));
    assert_eq!(fired.get(), 0, "nothing fires until the hold elapses");
    pump_timers();
    assert_eq!(fired.get(), 1, "the hold elapsed while the press was down");
    assert_eq!(robot.release_native(&row), Ok(TapMechanism::HitTest));
    pump_timers();
    assert_eq!(fired.get(), 1);
    robot.reset();
}
