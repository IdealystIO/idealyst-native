//! Scope-anchored scheduling helpers — verify that
//! [`runtime_core::after_ms_scoped`], [`runtime_core::raf_loop_scoped`],
//! and the `timeline!` macro cancel their underlying timers when the
//! surrounding reactive scope cleans up.
//!
//! These tests need a real (deferring) scheduler so we can observe
//! the cancel: the native-fallback path in `runtime_core::scheduling`
//! runs callbacks synchronously at construction, which short-circuits
//! the cancel-on-drop behavior we want to verify. Each `tests/*.rs`
//! file compiles as its own binary, so the `SCHEDULER` `OnceLock` set
//! here doesn't affect any other test's expectations.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Duration;

use runtime_core::animation::{AnimatedValue, TweenTo};
use runtime_core::scheduling::{
    install_scheduler, ScheduleHandle, Scheduler,
};
use runtime_core::{
    after_ms_scoped, animated, on_cleanup, raf_loop_scoped, timeline, Effect,
};

// =============================================================================
// Test scheduler — defers everything; tests drive it explicitly
// =============================================================================

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

#[derive(Default)]
struct State {
    next_id: u32,
    one_shot: HashMap<u32, Box<dyn FnOnce() + 'static>>,
    raf: HashMap<u32, Box<dyn FnMut() + 'static>>,
    /// Count of `cancel()` calls that hit a still-pending task (i.e.
    /// the task hadn't fired yet). Tests use this as the headline
    /// signal that a scope-anchored helper actually cancelled.
    cancels: u32,
    /// Count of one-shot tasks that successfully fired.
    fires: u32,
    /// Count of raf-loop ticks delivered across all loops.
    raf_ticks: u32,
}

/// Drive every still-pending one-shot to fire, then return the
/// count fired. Used by tests that want to observe "what happened
/// when we DIDN'T cancel".
fn fire_pending_one_shots() -> u32 {
    let tasks: Vec<Box<dyn FnOnce()>> = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.one_shot.drain().map(|(_, cb)| cb).collect()
    });
    let count = tasks.len() as u32;
    for cb in tasks {
        cb();
    }
    STATE.with(|s| s.borrow_mut().fires += count);
    count
}

/// Drive one round of every registered raf loop. Each loop's body
/// fires once. Counts the deliveries via `raf_ticks`.
fn tick_raf_loops_once() {
    // Drain so the iteration sees only this round; re-insert after.
    // `FnMut` body can call back into the scheduler (e.g. cancel),
    // so we hold no borrow across the user code.
    let loops: Vec<(u32, Box<dyn FnMut()>)> = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.raf.drain().collect()
    });
    for (id, mut cb) in loops {
        cb();
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.raf_ticks += 1;
            // Only re-insert if the body didn't cancel itself.
            // (Cancel removes from `raf`; absence here = cancelled.)
            // Since we drained, "still present" means re-inserted
            // by something — there shouldn't be a path that does
            // that, so unconditionally re-insert is fine.
            s.raf.insert(id, cb);
        });
    }
}

fn pending_one_shots() -> usize {
    STATE.with(|s| s.borrow().one_shot.len())
}

fn pending_raf_loops() -> usize {
    STATE.with(|s| s.borrow().raf.len())
}

fn cancel_count() -> u32 {
    STATE.with(|s| s.borrow().cancels)
}

fn next_id() -> u32 {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let id = s.next_id;
        s.next_id += 1;
        id
    })
}

struct TestScheduler;

// SAFETY: storage lives in a `thread_local!`, so a `TestScheduler`
// value is never actually used to cross threads. The `Send + Sync`
// trait bounds on `Scheduler` exist for backends like the web
// (which is single-threaded) and for storage in a `OnceLock`; we
// satisfy them at the type level here without using the value
// across threads in the test binary.
unsafe impl Send for TestScheduler {}
unsafe impl Sync for TestScheduler {}

impl Scheduler for TestScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        // Tests don't currently exercise microtasks; just run sync
        // to keep behavior predictable if some path lands here.
        f();
    }

    fn after_animation_frame(
        &self,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        let id = next_id();
        STATE.with(|s| {
            s.borrow_mut().one_shot.insert(id, f);
        });
        Box::new(TestHandle::OneShot(id))
    }

    fn after_ms(
        &self,
        _delay_ms: i32,
        f: Box<dyn FnOnce() + 'static>,
    ) -> Box<dyn ScheduleHandle> {
        // Tests don't care about the delay — `fire_pending_one_shots`
        // drains everything in registration order. This is what lets
        // a `timeline!` with multiple stages register without firing
        // until we explicitly drive the queue.
        let id = next_id();
        STATE.with(|s| {
            s.borrow_mut().one_shot.insert(id, f);
        });
        Box::new(TestHandle::OneShot(id))
    }

    fn raf_loop(&self, f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        let id = next_id();
        STATE.with(|s| {
            s.borrow_mut().raf.insert(id, f);
        });
        Box::new(TestHandle::Raf(id))
    }
}

enum TestHandle {
    OneShot(u32),
    Raf(u32),
}

impl ScheduleHandle for TestHandle {
    fn cancel(&mut self) {
        let id = match self {
            TestHandle::OneShot(id) => *id,
            TestHandle::Raf(id) => *id,
        };
        let removed = STATE.with(|s| {
            let mut s = s.borrow_mut();
            match self {
                TestHandle::OneShot(_) => s.one_shot.remove(&id).is_some(),
                TestHandle::Raf(_) => s.raf.remove(&id).is_some(),
            }
        });
        if removed {
            STATE.with(|s| s.borrow_mut().cancels += 1);
        }
    }
}

impl Drop for TestHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn install_test_scheduler() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        install_scheduler(Box::new(TestScheduler));
    });
}

fn reset_state() {
    STATE.with(|s| {
        *s.borrow_mut() = State::default();
    });
}

// =============================================================================
// Tests
// =============================================================================

#[test]
fn after_ms_scoped_cancels_on_effect_drop() {
    install_test_scheduler();
    reset_state();

    let fired = Rc::new(Cell::new(false));
    let fired_for_effect = fired.clone();
    {
        let _e = Effect::new(move || {
            let f = fired_for_effect.clone();
            after_ms_scoped(1000, move || f.set(true));
        });
        // Inside the live effect: one task pending, nothing fired yet.
        assert_eq!(pending_one_shots(), 1, "task should be pending");
        assert!(!fired.get(), "callback should not have fired yet");
        // Effect dropped at end of block.
    }

    // Scope cleanup must have cancelled the pending task.
    assert_eq!(pending_one_shots(), 0, "task should be cancelled");
    assert_eq!(cancel_count(), 1, "exactly one cancel should be recorded");

    // Driving the (now-empty) queue must not fire the callback.
    fire_pending_one_shots();
    assert!(
        !fired.get(),
        "callback must not fire after the surrounding scope was dropped"
    );
}

#[test]
fn after_ms_scoped_still_fires_while_scope_alive() {
    install_test_scheduler();
    reset_state();

    let fired = Rc::new(Cell::new(false));
    let fired_for_effect = fired.clone();
    let _e = Effect::new(move || {
        let f = fired_for_effect.clone();
        after_ms_scoped(1000, move || f.set(true));
    });

    // Driving the queue while the effect is still alive fires the
    // callback normally — the scope-anchor only cancels on cleanup.
    fire_pending_one_shots();
    assert!(fired.get(), "callback fires when scheduler dispatches it");
    drop(_e);
}

#[test]
fn raf_loop_scoped_stops_ticking_on_effect_drop() {
    install_test_scheduler();
    reset_state();

    let ticks = Rc::new(Cell::new(0u32));
    let ticks_for_effect = ticks.clone();
    {
        let _e = Effect::new(move || {
            let t = ticks_for_effect.clone();
            raf_loop_scoped(move || {
                t.set(t.get() + 1);
            });
        });
        assert_eq!(pending_raf_loops(), 1, "loop should be registered");

        // Tick the loop a couple of times — body should fire each time.
        tick_raf_loops_once();
        tick_raf_loops_once();
        assert_eq!(ticks.get(), 2, "raf body fires while scope alive");
        // Effect dropped at end of block.
    }

    assert_eq!(
        pending_raf_loops(),
        0,
        "raf loop should be cancelled on scope cleanup"
    );
    assert_eq!(cancel_count(), 1);

    // Ticking again after the scope drops MUST NOT increment the
    // counter — the loop is gone.
    tick_raf_loops_once();
    assert_eq!(
        ticks.get(),
        2,
        "no further raf ticks after the surrounding scope dropped"
    );
}

#[test]
fn timeline_tasks_cancel_on_effect_drop() {
    install_test_scheduler();
    reset_state();

    // Three-stage timeline with a no-op tween at each stage. We
    // don't need the AVs to actually animate anything — we're
    // testing the schedule/cancel plumbing of the macro.
    let av_a = animated!(0.0_f32);
    let av_b = animated!(0.0_f32);
    let av_c = animated!(0.0_f32);
    {
        let _e = Effect::new(move || {
            timeline! {
                100 => {
                    av_a: TweenTo::new(1.0, Duration::from_millis(50)).ease_out(),
                },
                200 => {
                    av_b: TweenTo::new(1.0, Duration::from_millis(50)).ease_out(),
                },
                300 => {
                    av_c: TweenTo::new(1.0, Duration::from_millis(50)).ease_out(),
                },
            };
        });
        assert_eq!(pending_one_shots(), 3, "timeline registered all three tasks");
    }

    // Scope drop cancels every pending task in the timeline.
    assert_eq!(pending_one_shots(), 0, "all timeline tasks cancelled");
    assert_eq!(cancel_count(), 3, "three cancels recorded");
}

#[test]
fn timeline_tasks_fire_while_scope_alive() {
    install_test_scheduler();
    reset_state();

    let av = animated!(0.0_f32);
    let _e = Effect::new(move || {
        timeline! {
            100 => {
                av: TweenTo::new(1.0, Duration::from_millis(50)).ease_out(),
            },
        };
    });
    assert_eq!(pending_one_shots(), 1);

    // Driving the queue while the scope is alive lets the timeline
    // task fire normally.
    fire_pending_one_shots();
    assert_eq!(pending_one_shots(), 0, "task drained out by firing");
    drop(_e);
}

#[test]
fn after_ms_scoped_outside_scope_is_a_noop() {
    install_test_scheduler();
    reset_state();

    // No surrounding Effect / scope: the `on_cleanup` call inside
    // `after_ms_scoped` silently drops its captured closure (per
    // the standard `on_cleanup` contract). Dropping the closure
    // drops the captured `ScheduledTask`, which cancels the
    // underlying timer. Net effect: outside a scope, the helper
    // is a no-op. Authors who need a free-floating timer should
    // call plain `after_ms` directly.
    let fired = Rc::new(Cell::new(false));
    let f = fired.clone();
    after_ms_scoped(1000, move || f.set(true));

    assert_eq!(
        pending_one_shots(),
        0,
        "no-op outside scope: registered timer is immediately cancelled"
    );
    fire_pending_one_shots();
    assert!(!fired.get(), "callback never fires outside a scope");
}

#[test]
fn timeline_outside_scope_is_a_noop() {
    install_test_scheduler();
    reset_state();

    // Same shape as `after_ms_scoped` outside a scope — the macro's
    // internal `on_cleanup` drops the task `Vec`, cancelling every
    // pending dispatch immediately.
    let av = animated!(0.0_f32);
    timeline! {
        100 => {
            av: TweenTo::new(1.0, Duration::from_millis(50)).ease_out(),
        },
    };

    assert_eq!(
        pending_one_shots(),
        0,
        "no-op outside scope: timeline tasks were immediately cancelled"
    );
}

#[test]
fn nested_effect_inherits_scope_anchor() {
    install_test_scheduler();
    reset_state();

    // An effect inside an effect creates a nested scope; cleanup
    // on the inner effect cancels its scoped timers without touching
    // the outer.
    let outer_fired = Rc::new(Cell::new(false));
    let inner_fired = Rc::new(Cell::new(false));
    let outer_fired_for_outer = outer_fired.clone();
    let inner_fired_for_outer = inner_fired.clone();
    let _outer = Effect::new(move || {
        let outer_fired = outer_fired_for_outer.clone();
        after_ms_scoped(500, move || outer_fired.set(true));

        let inner_fired = inner_fired_for_outer.clone();
        let _inner = Effect::new(move || {
            let f = inner_fired.clone();
            after_ms_scoped(500, move || f.set(true));
        });
        // _inner dropped here — its scoped task cancels.
    });

    assert_eq!(
        cancel_count(),
        1,
        "inner scope cancelled its single task on drop"
    );
    assert_eq!(
        pending_one_shots(),
        1,
        "outer scope's task is still pending"
    );

    // Firing what remains hits only the outer.
    fire_pending_one_shots();
    assert!(outer_fired.get(), "outer task fires");
    assert!(
        !inner_fired.get(),
        "inner task does NOT fire — it was cancelled when its scope dropped"
    );
    drop(_outer);
}

#[test]
fn on_cleanup_still_works_alongside_scoped_helpers() {
    // Sanity check: an author who wires their OWN `on_cleanup`
    // alongside the new helpers should not interfere with the
    // helpers' internal cleanup.
    install_test_scheduler();
    reset_state();

    let trace: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
    let trace_for_effect = trace.clone();
    {
        let _e = Effect::new(move || {
            let t = trace_for_effect.clone();
            on_cleanup(move || t.borrow_mut().push("user-cleanup"));
            after_ms_scoped(500, || {});
            raf_loop_scoped(|| {});
        });
    }

    // Cleanups run in LIFO order (per the existing `on_cleanup`
    // semantics): the helper-installed cleanups registered AFTER
    // the user one, so they fire first.
    let order = trace.borrow().clone();
    assert!(
        order.contains(&"user-cleanup"),
        "user-supplied cleanup must still fire"
    );
    // All three scope-anchored handles were cancelled.
    assert_eq!(cancel_count(), 2, "after_ms_scoped + raf_loop_scoped both cancelled");
}

#[test]
fn explicit_cancel_inside_scope_is_idempotent_with_scope_drop() {
    install_test_scheduler();
    reset_state();

    let fired = Rc::new(Cell::new(false));
    let fired_for_effect = fired.clone();
    {
        let _e = Effect::new(move || {
            let f = fired_for_effect.clone();
            after_ms_scoped(1000, move || f.set(true));
        });
        // Force-drain everything BEFORE the scope ends. This proves
        // the scope's cleanup gracefully handles handles whose
        // underlying task already fired (the cancel is a no-op).
        fire_pending_one_shots();
        assert!(fired.get());
    }

    // No additional cancellations should have been recorded by the
    // scope cleanup (the task was already consumed).
    assert_eq!(
        cancel_count(),
        0,
        "scope cleanup over a fired task is a no-op cancel"
    );
}

#[test]
fn raf_loop_scoped_inside_after_ms_scoped_keeps_running() {
    // Regression: when the welcome scene started its raf-driven
    // pulse via `raf_loop_scoped` from inside an `after_ms_scoped`
    // callback, the loop was being cancelled before any frame fired.
    // Cause: the deferred callback ran outside any active scope, so
    // the inner `on_cleanup` saw an empty stack and dropped the
    // RafLoop handle immediately. Fix: each `*_scoped` helper now
    // captures the active scope stack at registration and re-enters
    // it when the callback fires, so nested helpers attach to the
    // same scope.
    install_test_scheduler();
    reset_state();

    let ticks = Rc::new(Cell::new(0u32));
    let ticks_for_effect = ticks.clone();
    {
        let _e = Effect::new(move || {
            let t = ticks_for_effect.clone();
            after_ms_scoped(500, move || {
                raf_loop_scoped(move || {
                    t.set(t.get() + 1);
                });
            });
        });

        // Fire the deferred shot — this is what installs the raf loop.
        fire_pending_one_shots();
        assert_eq!(
            pending_raf_loops(),
            1,
            "raf loop must survive past the after_ms_scoped callback's return — \
             a stale build would see 0 here because the inner cleanup fired \
             instantly and cancelled the loop"
        );

        // Now actually tick — verifies frames flow through.
        tick_raf_loops_once();
        tick_raf_loops_once();
        assert_eq!(ticks.get(), 2, "raf body fires while outer scope alive");
        // Effect dropped at end of block; both timers should die.
    }

    assert_eq!(
        pending_raf_loops(),
        0,
        "raf loop cancelled when its owning effect drops",
    );
    // Cancels: 0 from after_ms (it already fired), 1 from the raf loop.
    assert_eq!(cancel_count(), 1);
}
