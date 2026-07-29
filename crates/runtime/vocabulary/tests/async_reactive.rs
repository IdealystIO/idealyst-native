//! `glue::{resource, mutation}` — the new-core async-reactive mirrors
//! (`src/async_reactive.rs`).
//!
//! Own integration test (own process) because it installs a buffering
//! test executor through the global first-install-wins
//! `runtime_core::driver::install_async_executor` slot — the same
//! isolation rationale as `tests/scoped_scheduling.rs` and its
//! scheduler slot. The executor queues spawned futures in a
//! thread-local and only makes progress when a test pumps it, which is
//! what lets these tests pin the event-boundary contract: an async
//! completion STAGES its writes (nothing observable) until the owning
//! world flushes — in production the host's post-dispatch hook fires
//! that flush after every future poll (`backend-web/src/dispatch_hook.rs`);
//! here the tests flush explicitly so the boundary is visible.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use runtime_core::driver::{install_async_executor, AsyncExecutor};
use runtime_vocabulary::glue::{self, NetworkState};
use runtime_world::{collect_owned, effect, signal, World};

// ===========================================================================
// Test executor — buffering, thread-local, manually pumped.
// ===========================================================================

thread_local! {
    static TASKS: RefCell<Vec<Pin<Box<dyn Future<Output = ()> + 'static>>>> =
        const { RefCell::new(Vec::new()) };
}

struct TestExecutor;

// SAFETY: zero-sized; all live state is thread-local (each test thread
// pumps only its own queue) — the TestScheduler precedent in
// tests/scoped_scheduling.rs.
unsafe impl Send for TestExecutor {}
unsafe impl Sync for TestExecutor {}

impl AsyncExecutor for TestExecutor {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
        TASKS.with(|t| t.borrow_mut().push(future));
    }
}

fn ensure_test_executor() {
    install_async_executor(Box::new(TestExecutor));
}

/// Poll every queued future once (with a noop waker — gate futures are
/// re-polled by the next pump, not by wakeups). Completed futures are
/// dropped; pending ones are retained. Returns how many completed.
fn pump() -> usize {
    let mut tasks = TASKS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    let waker = std::task::Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut completed = 0;
    tasks.retain_mut(|f| match f.as_mut().poll(&mut cx) {
        Poll::Ready(()) => {
            completed += 1;
            false
        }
        Poll::Pending => true,
    });
    // Put survivors back, keeping anything spawned during the polls.
    TASKS.with(|t| {
        let mut q = t.borrow_mut();
        tasks.append(&mut q);
        *q = tasks;
    });
    completed
}

// ===========================================================================
// Gate — a future completed manually by the test.
// ===========================================================================

struct Gate<T, E> {
    cell: Rc<RefCell<Option<Result<T, E>>>>,
}

impl<T, E> Clone for Gate<T, E> {
    fn clone(&self) -> Self {
        Gate { cell: self.cell.clone() }
    }
}

impl<T, E> Gate<T, E> {
    fn new() -> Self {
        Gate { cell: Rc::new(RefCell::new(None)) }
    }
    fn complete(&self, r: Result<T, E>) {
        *self.cell.borrow_mut() = Some(r);
    }
    fn fut(&self) -> GateFut<T, E> {
        GateFut(self.cell.clone())
    }
}

struct GateFut<T, E>(Rc<RefCell<Option<Result<T, E>>>>);

impl<T: Unpin, E: Unpin> Future for GateFut<T, E> {
    type Output = Result<T, E>;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.0.borrow_mut().take() {
            Some(r) => Poll::Ready(r),
            None => Poll::Pending,
        }
    }
}

// ===========================================================================
// resource
// ===========================================================================

/// The event-boundary contract: the async completion's write STAGES
/// (reads keep the committed value) and becomes observable only at the
/// owning world's flush — in production, the post-dispatch hook's job.
#[test]
fn resource_completion_stages_until_flush_then_commits() {
    ensure_test_executor();
    let world = World::new();
    world.enter(|| {
        let gate: Gate<i32, &'static str> = Gate::new();
        let g = gate.clone();
        let dep = signal(0u32);
        let r = glue::resource(dep, move |_, _cancel| g.fut());

        world.flush(); // commit the initial loading state
        assert!(r.loading(), "eager fetch in flight");
        assert_eq!(r.data(), None);
        assert_eq!(r.network_state(), NetworkState::Loading);

        gate.complete(Ok(42));
        assert_eq!(pump(), 1, "completion future settled");

        // Staged, not committed: the boundary under test.
        assert_eq!(r.data(), None, "completion write must stage until flush");
        assert!(r.loading(), "loading flip is staged too");

        world.flush();
        assert_eq!(r.data(), Some(42));
        assert!(!r.loading());
        assert_eq!(r.error(), None);
        assert_eq!(r.network_state(), NetworkState::Success(42));
    });
}

/// Dep change cancels the in-flight fetch (token + on_cancel callback)
/// and the superseded fetch's late completion is discarded by the
/// sequence guard — old-core parity for the core race.
#[test]
fn resource_dep_change_cancels_and_discards_stale_result() {
    ensure_test_executor();
    let world = World::new();
    world.enter(|| {
        let dep = signal(1u32);
        let gates: Rc<RefCell<Vec<Gate<u32, &'static str>>>> = Rc::new(RefCell::new(Vec::new()));
        let tokens: Rc<RefCell<Vec<glue::ResourceCancel>>> = Rc::new(RefCell::new(Vec::new()));
        let cancel_hits = Rc::new(Cell::new(0u32));

        let (ga, to, ch) = (gates.clone(), tokens.clone(), cancel_hits.clone());
        let r = glue::resource(dep, move |_k, cancel| {
            let gate = Gate::new();
            ga.borrow_mut().push(gate.clone());
            let hits = ch.clone();
            cancel.on_cancel(move || hits.set(hits.get() + 1));
            to.borrow_mut().push(cancel);
            gate.fut()
        });
        world.flush();
        assert_eq!(gates.borrow().len(), 1);
        assert!(!tokens.borrow()[0].is_cancelled());

        dep.set(2);
        world.flush(); // effect re-run: cancel fetch 1, issue fetch 2
        assert_eq!(gates.borrow().len(), 2, "dep change issued a fresh fetch");
        assert!(tokens.borrow()[0].is_cancelled(), "previous fetch's token cancelled");
        assert_eq!(cancel_hits.get(), 1, "on_cancel callback fired once");
        assert!(!tokens.borrow()[1].is_cancelled());

        // The OLD fetch settles late: sequence guard discards it.
        gates.borrow()[0].complete(Ok(111));
        pump();
        world.flush();
        assert_eq!(r.data(), None, "stale result discarded");
        assert!(r.loading(), "fetch 2 still in flight");

        gates.borrow()[1].complete(Ok(222));
        pump();
        world.flush();
        assert_eq!(r.data(), Some(222), "newest fetch wins");
        assert!(!r.loading());
    });
}

/// A failed refetch keeps the previous data (no empty-flash) and
/// surfaces the error — the old-core stale-data-retention contract.
#[test]
fn resource_error_keeps_data_from_prior_success() {
    ensure_test_executor();
    let world = World::new();
    world.enter(|| {
        let dep = signal(0u32);
        let gates: Rc<RefCell<Vec<Gate<i32, &'static str>>>> = Rc::new(RefCell::new(Vec::new()));
        let ga = gates.clone();
        let r = glue::resource(dep, move |_k, _c| {
            let gate = Gate::new();
            ga.borrow_mut().push(gate.clone());
            gate.fut()
        });
        world.flush();
        gates.borrow()[0].complete(Ok(10));
        pump();
        world.flush();
        assert_eq!(r.data(), Some(10));

        dep.set(1);
        world.flush();
        // Error clears at fetch start, data retained while loading.
        let mid = r.state();
        assert!(mid.loading && mid.error.is_none() && mid.data == Some(10));

        gates.borrow()[1].complete(Err("boom"));
        pump();
        world.flush();
        assert_eq!(r.data(), Some(10), "data retained across a failed refetch");
        assert_eq!(r.error(), Some("boom"));
        assert!(!r.loading());
        assert_eq!(
            r.network_state(),
            NetworkState::Error("boom"),
            "error beats stale success in the collapsed projection"
        );
    });
}

/// `refetch()` re-runs the fetcher without a dep change; multiple
/// refetches in one turn COALESCE into one fetch (the documented
/// staged-counter divergence — the old core issued N fetches whose
/// first N−1 results the stale guard discarded anyway).
#[test]
fn resource_refetch_reruns_fetcher_and_coalesces_per_flush() {
    ensure_test_executor();
    let world = World::new();
    world.enter(|| {
        let dep = signal(0u32);
        let calls = Rc::new(Cell::new(0u32));
        let ca = calls.clone();
        let r: glue::Resource<u32, &'static str> = glue::resource(dep, move |_k, _c| {
            let n = ca.get() + 1;
            ca.set(n);
            let gate = Gate::new();
            gate.complete(Ok(n)); // settle on first poll
            gate.fut()
        });
        world.flush();
        pump();
        world.flush();
        assert_eq!(r.data(), Some(1));

        r.refetch();
        r.refetch(); // same turn: composes into ONE re-run
        world.flush();
        assert_eq!(calls.get(), 2, "two same-turn refetches coalesced into one fetch");
        pump();
        world.flush();
        assert_eq!(r.data(), Some(2));

        r.refetch();
        world.flush();
        pump();
        world.flush();
        assert_eq!(r.data(), Some(3), "later refetch fetches again");
        assert_eq!(calls.get(), 3);
    });
}

/// Reactive consumers of the accessors re-fire when the state commits —
/// the "resource is a signal" half of the contract.
#[test]
fn resource_reads_subscribe_reactive_consumers() {
    ensure_test_executor();
    let world = World::new();
    world.enter(|| {
        let dep = signal(0u32);
        let gate: Gate<i32, &'static str> = Gate::new();
        let g = gate.clone();
        let r = glue::resource(dep, move |_k, _c| g.fut());

        let observed = Rc::new(Cell::new(-1i32));
        let o = observed.clone();
        let _e = effect(move || {
            o.set(r.data().unwrap_or(0));
        });
        world.flush();
        assert_eq!(observed.get(), 0, "no data yet");

        gate.complete(Ok(7));
        pump();
        world.flush();
        assert_eq!(observed.get(), 7, "consumer effect re-fired on commit");
    });
}

/// THE new-core hazard this module exists to guard (module docs): the
/// owning scope unmounts while a fetch is in flight; the completion
/// lands afterwards. The kernel panics on stale-handle writes, so the
/// completion must consult the cancel token and drop itself silently.
#[test]
fn regression_teardown_drops_pending_completion_safely() {
    ensure_test_executor();
    let world = World::new();
    world.enter(|| {
        let gate: Gate<i32, &'static str> = Gate::new();
        let tokens: Rc<RefCell<Vec<glue::ResourceCancel>>> = Rc::new(RefCell::new(Vec::new()));
        let (g, to) = (gate.clone(), tokens.clone());
        let (_r, owned) = collect_owned(|| {
            let dep = signal(0u32);
            glue::resource(dep, move |_k, cancel| {
                to.borrow_mut().push(cancel);
                g.fut()
            })
        });
        world.flush();
        assert!(!tokens.borrow()[0].is_cancelled());

        // Component unmount: slots freed, cancel fired via the inner
        // effect's cleanup.
        drop(owned);
        assert!(
            tokens.borrow()[0].is_cancelled(),
            "scope teardown must fire the fetch's cancel token"
        );

        // The completion lands AFTER teardown. Un-guarded, this write
        // would hit a freed slot and abort with the kernel's
        // stale-handle panic — surviving the pump IS the assertion.
        gate.complete(Ok(1));
        assert_eq!(pump(), 1, "completion future settled without touching freed state");
        world.flush();
    });
}

/// The dead-world half of the same hazard class: the whole `World`
/// (an unmounted app, a finished SSR request) is gone before the
/// completion lands. World teardown runs effect cleanups (cancel
/// fires), and kernel writes to a dead world are silent no-ops.
#[test]
fn regression_dead_world_after_async_completion_is_noop() {
    ensure_test_executor();
    let gate: Gate<i32, &'static str> = Gate::new();
    {
        let world = World::new();
        world.enter(|| {
            let g = gate.clone();
            let dep = signal(0u32);
            let _r = glue::resource(dep, move |_k, _c| g.fut());
            world.flush();
        });
    } // world drops with the fetch pending

    gate.complete(Ok(9));
    assert_eq!(pump(), 1, "completion settled against a dead world without panicking");
}

/// A resource created inside an EFFECT body anchors to that effect: the
/// re-run tears the previous instance down (its fetch cancels, its
/// slots free) before a fresh one is built — the old-core "scope
/// adopts, disposes on re-run" contract via `anchor_to_scope`.
#[test]
fn resource_created_inside_effect_dies_with_the_rerun() {
    ensure_test_executor();
    let world = World::new();
    world.enter(|| {
        let dep = signal(0u32);
        let tokens: Rc<RefCell<Vec<glue::ResourceCancel>>> = Rc::new(RefCell::new(Vec::new()));
        let gates: Rc<RefCell<Vec<Gate<i32, &'static str>>>> = Rc::new(RefCell::new(Vec::new()));
        let (to, ga) = (tokens.clone(), gates.clone());
        let (_, _owned) = collect_owned(|| {
            let _ = effect(move || {
                let _ = dep.get();
                let (to2, ga2) = (to.clone(), ga.clone());
                let _r: glue::Resource<i32, &'static str> =
                    glue::resource(dep, move |_k, cancel| {
                        let gate = Gate::new();
                        ga2.borrow_mut().push(gate.clone());
                        to2.borrow_mut().push(cancel);
                        gate.fut()
                    });
            });
        });
        world.flush();
        assert_eq!(tokens.borrow().len(), 1);

        dep.set(1);
        world.flush(); // outer effect re-runs → previous resource torn down
        assert!(
            tokens.borrow()[0].is_cancelled(),
            "previous run's resource cancelled by the outer effect's re-run"
        );

        // Its late completion must be inert (freed slots).
        gates.borrow()[0].complete(Ok(5));
        pump();
        world.flush();
    });
}

/// Creation outside any `World::enter` panics — the glue-wide posture
/// (worlds are transient; there is no thread-lifetime home). Divergence
/// from the old core's `persist()` documented in the module/migration
/// docs.
#[test]
#[should_panic(expected = "outside World::enter")]
fn resource_outside_world_panics() {
    ensure_test_executor();
    let world = World::new();
    let dep = world.enter(|| signal(0u32));
    // `world` alive (the dep handle must not be dead) but NOT entered.
    let _r: glue::Resource<i32, &'static str> =
        glue::resource(dep, |_k, _c| GateFut::<i32, &'static str>(Rc::new(RefCell::new(None))));
}

// ===========================================================================
// mutation
// ===========================================================================

/// Fresh mutation is Idle (not loading); `trigger` drives
/// loading → success through the staged-commit boundary; triggering
/// from OUTSIDE `World::enter` (the event-handler surface) works —
/// writes route to the signal's own world.
#[test]
fn mutation_trigger_success_commits_on_flush() {
    ensure_test_executor();
    let world = World::new();
    let gate: Gate<i32, &'static str> = Gate::new();
    let m: glue::Mutation<i32, i32, &'static str> = world.enter(|| {
        let g = gate.clone();
        glue::mutation(move |x: i32| {
            let g2 = g.clone();
            async move {
                let base = g2.fut().await?;
                Ok(base + x)
            }
        })
    });
    world.flush();
    assert!(!m.loading(), "fresh mutation must not report loading");
    assert_eq!(m.network_state(), NetworkState::Idle, "never-triggered projects to Idle");

    // Event boundary: trigger outside enter, exactly like a handler.
    m.trigger(2);
    world.flush();
    assert!(m.loading());
    assert_eq!(m.data(), None);

    gate.complete(Ok(40));
    assert_eq!(pump(), 1);
    assert_eq!(m.data(), None, "completion write must stage until flush");
    world.flush();
    assert_eq!(m.data(), Some(42));
    assert_eq!(m.error(), None);
    assert!(!m.loading());
    assert_eq!(m.network_state(), NetworkState::Success(42));
}

/// A failed trigger keeps the prior success's data (optimistic-UI
/// affordance) and populates the error.
#[test]
fn mutation_error_keeps_prior_data() {
    ensure_test_executor();
    let world = World::new();
    world.enter(|| {
        let gates: Rc<RefCell<Vec<Gate<i32, &'static str>>>> = Rc::new(RefCell::new(Vec::new()));
        let ga = gates.clone();
        let m: glue::Mutation<i32, i32, &'static str> = glue::mutation(move |_x: i32| {
            let gate = Gate::new();
            ga.borrow_mut().push(gate.clone());
            gate.fut()
        });
        m.trigger(1);
        gates.borrow()[0].complete(Ok(10));
        pump();
        world.flush();
        assert_eq!(m.data(), Some(10));

        m.trigger(2);
        gates.borrow()[1].complete(Err("boom"));
        pump();
        world.flush();
        assert_eq!(m.data(), Some(10), "data retained across a failed trigger");
        assert_eq!(m.error(), Some("boom"));
        assert!(!m.loading());
    });
}

/// Back-to-back triggers: the superseded first trigger's completion is
/// discarded even when it settles LAST (sequence guard).
#[test]
fn mutation_stale_trigger_result_discarded() {
    ensure_test_executor();
    let world = World::new();
    world.enter(|| {
        let gates: Rc<RefCell<Vec<Gate<i32, &'static str>>>> = Rc::new(RefCell::new(Vec::new()));
        let ga = gates.clone();
        let m: glue::Mutation<i32, i32, &'static str> = glue::mutation(move |_x: i32| {
            let gate = Gate::new();
            ga.borrow_mut().push(gate.clone());
            gate.fut()
        });
        m.trigger(1);
        m.trigger(2);
        // Slow first trigger settles after the fast second one.
        gates.borrow()[1].complete(Ok(222));
        pump();
        gates.borrow()[0].complete(Ok(111));
        pump();
        world.flush();
        assert_eq!(m.data(), Some(222), "only the newest trigger's result applies");
    });
}

/// `reset` clears to Idle and invalidates the in-flight trigger — its
/// eventual completion must not resurrect state.
#[test]
fn mutation_reset_clears_and_invalidates_inflight() {
    ensure_test_executor();
    let world = World::new();
    world.enter(|| {
        let gate: Gate<i32, &'static str> = Gate::new();
        let g = gate.clone();
        let m: glue::Mutation<i32, i32, &'static str> = glue::mutation(move |_x: i32| g.fut());
        m.trigger(1);
        world.flush();
        assert!(m.loading());

        m.reset();
        world.flush();
        assert!(!m.loading());
        assert_eq!(m.data(), None);

        gate.complete(Ok(5));
        pump();
        world.flush();
        assert_eq!(m.data(), None, "invalidated trigger's completion stays discarded");
        assert_eq!(m.network_state(), NetworkState::Idle);
    });
}

/// The mutation half of the teardown hazard: scope unmounts mid-flight;
/// the completion must consult the liveness sentinel instead of writing
/// through the freed state slot (which would panic).
#[test]
fn regression_mutation_completion_after_unmount_is_dropped() {
    ensure_test_executor();
    let world = World::new();
    world.enter(|| {
        let gate: Gate<i32, &'static str> = Gate::new();
        let g = gate.clone();
        let (m, owned) = collect_owned(|| {
            let m: glue::Mutation<i32, i32, &'static str> = glue::mutation(move |_x: i32| g.fut());
            m.trigger(1);
            m
        });
        world.flush();

        drop(owned); // unmount: state slot freed, sentinel flips

        gate.complete(Ok(1));
        assert_eq!(pump(), 1, "completion settled without touching freed state");
        world.flush();
        drop(m); // handle outlives the slot harmlessly (no reads)
    });
}

/// `run` returns the result inline while updating the state signal the
/// same way `trigger` does.
#[test]
fn mutation_run_returns_result_inline() {
    ensure_test_executor();
    let world = World::new();
    world.enter(|| {
        let gate: Gate<i32, &'static str> = Gate::new();
        let g = gate.clone();
        let m: glue::Mutation<i32, i32, &'static str> = glue::mutation(move |x: i32| {
            let g2 = g.clone();
            async move {
                let base = g2.fut().await?;
                Ok(base + x)
            }
        });

        let mut fut = Box::pin(m.run(1));
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        assert!(fut.as_mut().poll(&mut cx).is_pending(), "gated handler still in flight");
        world.flush();
        assert!(m.loading(), "run marks loading like trigger");

        gate.complete(Ok(41));
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(r) => assert_eq!(r, Ok(42), "inline result returned"),
            Poll::Pending => panic!("gate completed; run must settle"),
        }
        world.flush();
        assert_eq!(m.data(), Some(42), "state signal updated too");
    });
}
