//! `scoped_scheduling` — the new-core anchors behind glue's
//! `after_ms_scoped` / `raf_loop_scoped` / `session::after_ms` and the
//! vocabulary `timeline!` mirror.
//!
//! Own integration test (own process) because it installs a buffering
//! test scheduler through the global first-install-wins
//! `runtime_shared::scheduling::install_scheduler` slot — the same
//! isolation rationale as the gpu backend's newcore suite, which this
//! harness mirrors.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use runtime_shared::scheduling::{install_scheduler, ScheduleHandle, Scheduler};
use runtime_vocabulary::glue;
use runtime_world::{collect_owned, effect, signal, World};

// ===========================================================================
// Test scheduler — buffering, thread-local, manually pumped (the gpu
// newcore suite's shape; raf_loop additionally retained + cancellable
// because raf_loop_scoped is under test here).
// ===========================================================================

thread_local! {
    static MICROTASKS: RefCell<VecDeque<Box<dyn FnOnce() + 'static>>> =
        RefCell::new(VecDeque::new());
    static TIMERS: RefCell<VecDeque<Box<dyn FnOnce() + 'static>>> =
        RefCell::new(VecDeque::new());
    /// Live raf-loop bodies. `Some` = alive; a cancelled handle
    /// tombstones its slot.
    static RAF_LOOPS: RefCell<Vec<Option<Box<dyn FnMut() + 'static>>>> =
        RefCell::new(Vec::new());
}

struct TestScheduler;

struct InertHandle;
impl ScheduleHandle for InertHandle {
    fn cancel(&mut self) {}
}

struct RafHandle(usize);
impl ScheduleHandle for RafHandle {
    fn cancel(&mut self) {
        let _ = RAF_LOOPS.try_with(|l| {
            if let Some(slot) = l.borrow_mut().get_mut(self.0) {
                *slot = None;
            }
        });
    }
}

// SAFETY: zero-sized; all live state is thread-local (the winit/test
// scheduler precedent).
unsafe impl Send for TestScheduler {}
unsafe impl Sync for TestScheduler {}

impl Scheduler for TestScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        MICROTASKS.with(|q| q.borrow_mut().push_back(f));
    }

    fn after_animation_frame(&self, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        TIMERS.with(|q| q.borrow_mut().push_back(f));
        Box::new(InertHandle)
    }

    fn after_ms(&self, _delay_ms: i32, f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        TIMERS.with(|q| q.borrow_mut().push_back(f));
        Box::new(InertHandle)
    }

    fn raf_loop(&self, f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        let idx = RAF_LOOPS.with(|l| {
            let mut l = l.borrow_mut();
            l.push(Some(f));
            l.len() - 1
        });
        Box::new(RafHandle(idx))
    }

    fn drain_buffered_microtasks(&self) {
        loop {
            let next = MICROTASKS.with(|q| q.borrow_mut().pop_front());
            match next {
                Some(f) => f(),
                None => break,
            }
        }
    }
}

fn ensure_test_scheduler() {
    install_scheduler(Box::new(TestScheduler));
}

/// Dispatch every queued one-shot (`after_ms` / frame callbacks).
fn pump_timers() {
    loop {
        let next = TIMERS.with(|q| q.borrow_mut().pop_front());
        match next {
            Some(f) => f(),
            None => break,
        }
    }
}

/// Tick every LIVE raf loop once. Returns how many bodies ran.
fn tick_raf_loops() -> usize {
    let count = RAF_LOOPS.with(|l| l.borrow().len());
    let mut ran = 0;
    for i in 0..count {
        // Take the body out while it runs (it may cancel itself).
        let body = RAF_LOOPS.with(|l| l.borrow_mut()[i].take());
        if let Some(mut f) = body {
            f();
            ran += 1;
            RAF_LOOPS.with(|l| {
                let mut l = l.borrow_mut();
                if l[i].is_none() {
                    l[i] = Some(f);
                }
            });
        }
    }
    ran
}

/// Remove one queued one-shot from the scheduler and hand it back
/// UNQUEUED — the "the browser already dispatched this callback"
/// simulation. Cancellation can no longer reach it, so only the
/// framework's dead flag can keep it from running.
fn steal_one_timer() -> Box<dyn FnOnce() + 'static> {
    TIMERS
        .with(|q| q.borrow_mut().pop_front())
        .expect("a one-shot was queued")
}

/// Same for a raf loop: take the body out of the registry without
/// restoring it, so a later `cancel()` tombstones an empty slot.
fn steal_one_raf() -> Box<dyn FnMut() + 'static> {
    RAF_LOOPS
        .with(|l| {
            l.borrow_mut()
                .iter_mut()
                .find_map(|slot| slot.take())
        })
        .expect("a raf loop was registered")
}

// ===========================================================================
// Tests
// ===========================================================================

/// The effect-run anchor: a scoped timer registered inside an effect
/// body dies when that effect RE-RUNS (the old core's "timers die when
/// the surrounding `effect!` re-runs" contract) — and a queued dispatch
/// from before the re-run is inert (dead-flag, not just cancel).
#[test]
fn effect_anchored_timer_cancels_on_rerun() {
    ensure_test_scheduler();
    let world = World::new();
    let fired: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    world.enter(|| {
        let gen = signal(0u32);
        let fi = fired.clone();
        let (_, owned) = collect_owned(|| {
            let _ = effect(move || {
                let _g = gen.get(); // dependency: re-runs on set
                let fi2 = fi.clone();
                glue::after_ms_scoped(0, move || fi2.set(fi2.get() + 1));
            });
        });

        // First registration fires normally.
        pump_timers();
        assert_eq!(fired.get(), 1, "first-run timer fired");

        // Re-run replaces the anchor; the SECOND registration fires,
        // and only once (the first anchor's cleanup killed only its own
        // domain).
        gen.set(1);
        world.flush();
        pump_timers();
        assert_eq!(fired.get(), 2, "re-run's own timer fired once");

        // Re-run again but DON'T pump: the queued dispatch belongs to
        // an anchor that dies with the next re-run — it must be inert.
        gen.set(2);
        world.flush(); // queues run 3's timer
        gen.set(3);
        world.flush(); // kills run 3's anchor, queues run 4's timer
        pump_timers();
        assert_eq!(fired.get(), 3, "run 3's queued-but-killed timer stayed inert");

        drop(owned);
        gen.set(4);
        world.flush();
        pump_timers();
        assert_eq!(fired.get(), 3, "owner dropped: nothing further fires");
    });
}

/// The build/collector anchor + the deferred re-entry channel: a
/// `raf_loop_scoped` registered INSIDE a `session::after_ms` body (the
/// welcome app's exact nesting) anchors to the lifetime that registered
/// the timer — dropping the collected `Owned` freezes the loop.
#[test]
fn nested_raf_loop_inherits_the_registering_anchor() {
    ensure_test_scheduler();
    let world = World::new();
    let ticks: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    world.enter(|| {
        let ti = ticks.clone();
        let (_, owned) = collect_owned(|| {
            glue::session::after_ms(0, move || {
                let ti2 = ti.clone();
                glue::raf_loop_scoped(move || ti2.set(ti2.get() + 1));
            });
        });

        // Timer fires (build-anchored), installing the loop under the
        // SAME anchor via the deferred re-entry stack.
        pump_timers();
        tick_raf_loops();
        tick_raf_loops();
        assert_eq!(ticks.get(), 2, "nested loop runs while the anchor lives");

        // Killing the registering anchor freezes the nested loop too —
        // this is what stops the welcome sun/planet pulse when the
        // embedded simulator unmounts.
        drop(owned);
        tick_raf_loops();
        assert_eq!(ticks.get(), 2, "nested loop dead after the anchor's owner dropped");
    });
}

/// Outside any scope (no effect, no entered world, no deferred body)
/// the scoped variants are inert — old-core parity ("the captured task
/// is dropped immediately").
#[test]
fn outside_any_scope_scoped_variants_are_inert() {
    ensure_test_scheduler();
    let fired: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let fi = fired.clone();
    glue::after_ms_scoped(0, move || fi.set(true));
    let fi2 = fired.clone();
    glue::raf_loop_scoped(move || fi2.set(true));
    pump_timers();
    tick_raf_loops();
    assert!(!fired.get(), "no scope → no dispatch, mirroring the old core");
}

/// The `timeline!` mirror rides `glue::session::after_ms`: acts
/// registered from an effect body dispatch through the scheduler under
/// the new anchors, and acts whose owner died first stay inert. The
/// animate target is a duck-typed probe (the macro only needs
/// `.clone()` + `.animate(animator)`), keeping the assertion on the
/// SCHEDULING path — AV delivery is glue::animation's own concern.
#[test]
fn timeline_macro_fires_acts_through_the_new_anchors() {
    #[derive(Clone)]
    struct Probe(Rc<Cell<u32>>);
    impl Probe {
        fn animate(&self, delta: u32) {
            self.0.set(self.0.get() + delta);
        }
    }

    ensure_test_scheduler();
    let world = World::new();
    world.enter(|| {
        let hits: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        let probe = Probe(hits.clone());
        let (_, owned) = collect_owned(|| {
            let p = probe.clone();
            let _ = effect(move || {
                let opacity = p.clone();
                let scale = p.clone();
                runtime_vocabulary::timeline! {
                    0 => {
                        opacity: 1u32,
                        scale: 10u32,
                    },
                    5 => {
                        opacity: 100u32,
                    },
                };
            });
        });
        pump_timers(); // dispatch all three acts
        assert_eq!(hits.get(), 111, "every act fired exactly once");

        // A second effect run never happens (no deps), so re-arming
        // can't double-fire; the owner's death silences future acts.
        drop(owned);
        pump_timers();
        assert_eq!(hits.get(), 111);
    });
}

/// `glue::session::animated` hands out the SHARED session-registry
/// instance wrapped in the glue AV (hot-patch value survival — the
/// welcome scene's "acts don't replay on save" contract).
#[test]
fn session_animated_returns_the_shared_registry_instance() {
    let a = glue::session::animated("scoped_sched_shared_av", 0.25_f32);
    let b = glue::session::animated("scoped_sched_shared_av", 0.0_f32);
    a.set(0.75);
    assert_eq!(b.get(), 0.75, "same key → same underlying AV");
    // Drop the registry entry while the thread's TLS is still alive.
    glue::session::clear();
}

// ===========================================================================
// Teardown safety: the "browser already dispatched it" window.
//
// Ports of the old core's two documented CRASH regressions
// (`runtime-core/tests/scheduling_scoped.rs::
// {raf_loop_scoped,after_ms_scoped}_does_not_fire_after_scope_drop_even_if_
// browser_already_dispatched`). A host can dispatch a callback that was
// already queued for the current tick even after the handle was
// cancelled, so cancellation ALONE is not enough — hence the anchor's
// `dead` flag. Both bodies read a scope-owned signal, which is what
// turns "the body ran" into a hard abort rather than a wrong pixel: on
// the new kernel a read through a freed slot PANICS ("stale handle"),
// mirroring the old arena's "signal used after its scope was dropped".
// ===========================================================================

#[test]
fn regression_raf_loop_scoped_stays_inert_after_owner_drop_even_if_already_dispatched() {
    ensure_test_scheduler();
    let world = World::new();
    let ran_after_drop: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    world.enter(|| {
        let ra = ran_after_drop.clone();
        let (_, owned) = collect_owned(|| {
            // A subtree-owned signal the per-frame body reads — the
            // navigator-screen shape.
            let state = signal(0u32);
            glue::raf_loop_scoped(move || {
                // Reading a freed slot is the abort this guard prevents.
                let _ = state.get();
                ra.set(true);
            });
        });

        // The host dispatched this frame: steal the body so the
        // teardown's cancel cannot unqueue it.
        let mut already_dispatched = steal_one_raf();

        // The screen is released.
        drop(owned);

        // The stolen frame finally lands. It must bail at the top.
        already_dispatched();
        assert!(
            !ran_after_drop.get(),
            "a raf body must NOT run after its anchor's owner dropped, even though \
             the host had already dispatched the frame"
        );
    });
}

#[test]
fn regression_after_ms_scoped_stays_inert_after_owner_drop_even_if_already_dispatched() {
    ensure_test_scheduler();
    let world = World::new();
    let ran_after_drop: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    world.enter(|| {
        let ra = ran_after_drop.clone();
        let (_, owned) = collect_owned(|| {
            let state = signal(0u32);
            glue::after_ms_scoped(1000, move || {
                let _ = state.get();
                ra.set(true);
            });
        });

        let already_dispatched = steal_one_timer();
        drop(owned);
        already_dispatched();
        assert!(
            !ran_after_drop.get(),
            "a one-shot must NOT run after its anchor's owner dropped"
        );
    });
}

/// The other half of the pair: while the anchor's owner is alive, a
/// build-anchored one-shot DOES fire (the guard must not be a blanket
/// "never run"). Old-core twin: `after_ms_scoped_still_fires_while_scope_alive`.
#[test]
fn after_ms_scoped_fires_while_the_anchor_owner_is_alive() {
    ensure_test_scheduler();
    let world = World::new();
    let fired: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    world.enter(|| {
        let fi = fired.clone();
        let (_, owned) = collect_owned(|| {
            glue::after_ms_scoped(0, move || fi.set(fi.get() + 1));
        });
        pump_timers();
        assert_eq!(fired.get(), 1, "build-anchored one-shot fires while owned");
        drop(owned);
    });
}

/// `on_cleanup` inside an effect coexists with the scoped helpers
/// registered in the same body: both the author cleanup and the anchor
/// kill run, in that order, on re-run. Old-core twin:
/// `on_cleanup_still_works_alongside_scoped_helpers`.
#[test]
fn on_cleanup_coexists_with_scoped_helpers_in_the_same_effect() {
    ensure_test_scheduler();
    let world = World::new();
    let cleanups: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    let fired: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    world.enter(|| {
        let gen = signal(0u32);
        let cl = cleanups.clone();
        let fi = fired.clone();
        let (_, owned) = collect_owned(|| {
            let _ = effect(move || {
                let _g = gen.get();
                let cl2 = cl.clone();
                runtime_world::on_cleanup(move || cl2.set(cl2.get() + 1));
                let fi2 = fi.clone();
                glue::after_ms_scoped(0, move || fi2.set(fi2.get() + 1));
            });
        });

        // Re-run: the author cleanup runs AND the previous anchor dies.
        gen.set(1);
        world.flush();
        assert_eq!(cleanups.get(), 1, "author cleanup ran on re-run");
        pump_timers();
        assert_eq!(
            fired.get(),
            1,
            "only the live registration's timer fired (run 0's was killed)"
        );
        drop(owned);
    });
}
