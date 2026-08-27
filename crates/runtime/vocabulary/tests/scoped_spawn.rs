//! `spawn_then` — the scope-safe imperative spawn.
//!
//! Own integration test (own process) because it installs a buffering
//! test executor through the global first-install-wins
//! `runtime_shared::driver::install_async_executor` slot — the same
//! isolation rationale as `tests/async_reactive.rs` and
//! `tests/scoped_scheduling.rs`.
//!
//! What these pin, and why the mid-life teardown matters: the bug is a
//! continuation resuming into a scope that died WHILE the world stayed
//! alive (a route change, a host rebuild, a `switch` re-key). Tearing the
//! whole world down instead does not reproduce it — everything goes at
//! once and the write short-circuits for unrelated reasons. So every test
//! here mounts behind a structural hole and collapses only that hole.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use host_mock::Harness;
use runtime_shared::driver::{install_async_executor, AsyncExecutor};
use runtime_vocabulary::builders::view;
use runtime_vocabulary::scoped_spawn::spawn_then;
use runtime_world::{signal, Signal};

// ---------------------------------------------------------------------
// Test executor — buffering, thread-local, manually pumped.
// ---------------------------------------------------------------------

thread_local! {
    static TASKS: RefCell<Vec<Pin<Box<dyn Future<Output = ()> + 'static>>>> =
        const { RefCell::new(Vec::new()) };
}

struct TestExecutor;
// SAFETY: zero-sized; all live state is thread-local (each test thread
// pumps only its own queue) — the precedent in tests/async_reactive.rs.
unsafe impl Send for TestExecutor {}
unsafe impl Sync for TestExecutor {}

impl AsyncExecutor for TestExecutor {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
        TASKS.with(|t| t.borrow_mut().push(future));
    }
}

fn ensure_executor() {
    install_async_executor(Box::new(TestExecutor));
}

/// Poll every queued future once. Completed futures drop; pending ones
/// are retained.
fn pump() {
    let mut tasks = TASKS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    let waker = std::task::Waker::noop();
    let mut cx = Context::from_waker(waker);
    tasks.retain_mut(|f| f.as_mut().poll(&mut cx).is_pending());
    TASKS.with(|t| {
        let mut q = t.borrow_mut();
        tasks.append(&mut q);
        *q = tasks;
    });
}

/// A future the test completes by hand, so "in flight" is a real state.
#[derive(Clone)]
struct Gate(Rc<RefCell<bool>>);
impl Gate {
    fn new() -> Self {
        Gate(Rc::new(RefCell::new(false)))
    }
    fn complete(&self) {
        *self.0.borrow_mut() = true;
    }
}
impl Future for Gate {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if *self.0.borrow() { Poll::Ready(()) } else { Poll::Pending }
    }
}

// ---------------------------------------------------------------------
// Harness — a screen behind a structural hole.
// ---------------------------------------------------------------------

struct Screen {
    _realized: runtime_scene::Realized<host_mock::Node>,
    /// Collapsing the hole unmounts the screen; the world survives.
    shown: Signal<bool>,
    /// A signal owned by the SCREEN's scope — freed on unmount.
    scoped: Rc<RefCell<Option<Signal<i32>>>>,
}

/// Mount a screen owning one signal, and run `on_build` inside its scope
/// so a `spawn_then` registered there anchors to the screen.
fn mount_screen(h: &Harness, on_build: impl Fn(Signal<i32>) + 'static) -> Screen {
    let hole: Rc<RefCell<Option<Signal<bool>>>> = Rc::new(RefCell::new(None));
    let scoped: Rc<RefCell<Option<Signal<i32>>>> = Rc::new(RefCell::new(None));
    let hole_b = hole.clone();
    let scoped_b = scoped.clone();
    let realized = h.mount(h.world.enter(|| {
        let shown = signal(true);
        *hole_b.borrow_mut() = Some(shown);
        view()
            .child(move || {
                if shown.get() {
                    let s = signal(0i32);
                    *scoped_b.borrow_mut() = Some(s);
                    on_build(s);
                    view().build()
                } else {
                    view().build()
                }
            })
            .build()
    }));
    let shown = hole.borrow().expect("hole built");
    Screen { _realized: realized, shown, scoped }
}

// ---------------------------------------------------------------------

#[test]
fn callback_applies_its_writes_while_the_scope_is_alive() {
    ensure_executor();
    let h = Harness::new();
    let gate = Gate::new();
    let g = gate.clone();
    let screen = mount_screen(&h, move |scoped| {
        let g = g.clone();
        spawn_then(
            async move {
                g.await;
                7i32
            },
            move |v| scoped.set(v),
        );
    });
    h.world.flush();
    let scoped = screen.scoped.borrow().expect("mounted");

    pump(); // in flight
    assert_eq!(scoped.get(), 0, "nothing applied until the IO resolves");

    gate.complete();
    pump();
    h.world.flush();
    assert_eq!(scoped.get(), 7, "the callback ran and its write committed");
}

#[test]
fn regression_callback_is_skipped_when_the_scope_died_during_the_io() {
    // Under a raw `spawn_async` the tail writes a
    // freed slot and the app aborts with `stale-signal-handle`.
    ensure_executor();
    let h = Harness::new();
    let gate = Gate::new();
    let g = gate.clone();
    let ran: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let ran_c = ran.clone();
    let screen = mount_screen(&h, move |scoped| {
        let g = g.clone();
        let ran = ran_c.clone();
        spawn_then(
            async move {
                g.await;
                7i32
            },
            move |v| {
                *ran.borrow_mut() = true;
                scoped.set(v); // would abort if this ran
            },
        );
    });
    h.world.flush();
    pump(); // request in flight

    screen.shown.set(false); // navigate away — screen scope freed
    h.world.flush();

    gate.complete();
    pump(); // continuation resumes into a dead scope
    h.world.flush();

    assert!(!*ran.borrow(), "the callback must not run after its scope was torn down");
}

#[test]
fn the_io_still_completes_when_the_scope_dies() {
    // Liveness is checked AFTER the future resolves, so a save already in
    // flight is never abandoned — only its result is discarded. Cancelling
    // instead would lose user data (storage write-through, sync uploads).
    ensure_executor();
    let h = Harness::new();
    let gate = Gate::new();
    let g = gate.clone();
    let io_finished: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let io_c = io_finished.clone();
    let screen = mount_screen(&h, move |scoped| {
        let g = g.clone();
        let io = io_c.clone();
        spawn_then(
            async move {
                g.await;
                *io.borrow_mut() = true; // the "write reached the server"
                1i32
            },
            move |v| scoped.set(v),
        );
    });
    h.world.flush();
    pump();

    screen.shown.set(false);
    h.world.flush();

    gate.complete();
    pump();
    assert!(*io_finished.borrow(), "the in-flight IO must still run to completion");
}

#[test]
fn writes_land_all_or_nothing_across_mixed_ownership() {
    // The atomicity property a per-write guard cannot give: a callback
    // touching BOTH a root-owned signal and a screen-owned one must not
    // apply half of it.
    ensure_executor();
    let h = Harness::new();
    let root = h.world.enter(|| signal(0i32)); // outlives the screen
    let gate = Gate::new();
    let g = gate.clone();
    let screen = mount_screen(&h, move |scoped| {
        let g = g.clone();
        spawn_then(
            async move {
                g.await;
                5i32
            },
            move |v| {
                root.set(v); // would land under a per-write guard…
                scoped.set(v); // …while this one silently vanished
            },
        );
    });
    h.world.flush();
    pump();

    screen.shown.set(false);
    h.world.flush();

    gate.complete();
    pump();
    h.world.flush();

    assert_eq!(root.get(), 0, "no half-applied update: the global write must be skipped too");
}

#[test]
fn reads_inside_the_callback_are_safe() {
    // A stale READ can never be made benign — there is no value to
    // synthesize — so `data.get()` after an await is fatal under every
    // write policy. Inside the callback it is valid by construction.
    ensure_executor();
    let h = Harness::new();
    let gate = Gate::new();
    let g = gate.clone();
    let seen: Rc<RefCell<Option<i32>>> = Rc::new(RefCell::new(None));
    let seen_c = seen.clone();
    let screen = mount_screen(&h, move |scoped| {
        let g = g.clone();
        let seen = seen_c.clone();
        spawn_then(
            async move {
                g.await;
            },
            move |()| {
                *seen.borrow_mut() = Some(scoped.get()); // read, not write
            },
        );
    });
    h.world.flush();
    screen.scoped.borrow().expect("mounted").set(3);
    h.world.flush();

    pump();
    gate.complete();
    pump();
    h.world.flush();
    assert_eq!(*seen.borrow(), Some(3), "the callback read live state");
}

#[test]
fn outside_a_world_the_callback_still_runs() {
    // No scope to anchor to means nothing can flip the token, and a
    // backend-less unit test must still see its result applied — the same
    // posture `ScopeAlive::immortal` takes.
    ensure_executor();
    let done: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let d = done.clone();
    spawn_then(async { 1i32 }, move |_| *d.borrow_mut() = true);
    pump();
    assert!(*done.borrow(), "a task spawned outside any world still applies");
}

// ---------------------------------------------------------------------
// Handler-time spawning — the shape the build-time tests above MISS.
// ---------------------------------------------------------------------

/// Mount a pressable whose handler body runs `on_press`. The handler is
/// invoked by the backend later, with no collector on the stack — which is
/// where `ScopeAlive::current()` used to anchor to nothing.
fn mount_button(h: &Harness, on_press: impl Fn(Signal<i32>) + 'static) -> Screen {
    let hole: Rc<RefCell<Option<Signal<bool>>>> = Rc::new(RefCell::new(None));
    let scoped: Rc<RefCell<Option<Signal<i32>>>> = Rc::new(RefCell::new(None));
    let hole_b = hole.clone();
    let scoped_b = scoped.clone();
    let on_press = Rc::new(on_press);
    let realized = h.mount(h.world.enter(|| {
        let shown = signal(true);
        *hole_b.borrow_mut() = Some(shown);
        view()
            .child(move || {
                if shown.get() {
                    let s = signal(0i32);
                    *scoped_b.borrow_mut() = Some(s);
                    let on_press = on_press.clone();
                    view()
                        .children(vec![runtime_vocabulary::builders::pressable(move || {
                            on_press(s)
                        })
                        .build()])
                        .build()
                } else {
                    view().build()
                }
            })
            .build()
    }));
    let shown = hole.borrow().expect("hole built");
    Screen { _realized: realized, shown, scoped }
}

/// The dominant real shape: a save handler spawns, the screen navigates
/// away, the continuation lands after teardown.
///
/// Regression: `ScopeAlive::current()` called at HANDLER time found no
/// ownership collector (handlers run outside `World::enter`, so
/// `on_owned_drop` was inert), handed back a token that could never flip,
/// and `spawn_then` guarded nothing. The build-time tests above all passed
/// throughout, because a build DOES have a collector. Fixed by having each
/// guarded callback publish its own (mount-time, correctly anchored) token
/// as the ambient one while it runs.
#[test]
fn handler_spawned_task_dies_with_its_node() {
    ensure_executor();
    let h = Harness::new();
    let gate = Gate::new();
    let g = gate.clone();
    let ran: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let ran_c = ran.clone();

    let screen = mount_button(&h, move |scoped| {
        let g = g.clone();
        let ran = ran_c.clone();
        spawn_then(
            async move {
                g.await;
                7i32
            },
            move |v| {
                *ran.borrow_mut() = true;
                scoped.set(v); // would abort if this ran
            },
        );
    });
    h.world.flush();

    h.press_handler(0)(); // user taps Save
    h.world.flush();
    pump(); // request in flight

    screen.shown.set(false); // navigate away
    h.world.flush();

    gate.complete();
    pump();
    h.world.flush();

    assert!(!*ran.borrow(), "a task spawned from a handler must die with the handler's node");
}

/// The inverse: a handler-spawned task whose node SURVIVES must still
/// apply. Inheriting the handler's token must not make every such task
/// inert — the failure mode a naive "always treat handler tasks as dead"
/// fix would introduce.
#[test]
fn handler_spawned_task_applies_while_its_node_lives() {
    ensure_executor();
    let h = Harness::new();
    let gate = Gate::new();
    let g = gate.clone();

    let screen = mount_button(&h, move |scoped| {
        let g = g.clone();
        spawn_then(
            async move {
                g.await;
                7i32
            },
            move |v| scoped.set(v),
        );
    });
    h.world.flush();
    let scoped = screen.scoped.borrow().expect("mounted");

    h.press_handler(0)();
    h.world.flush();
    pump();

    gate.complete();
    pump();
    h.world.flush();
    assert_eq!(scoped.get(), 7, "the node is still mounted — the result must apply");
}

// ---------------------------------------------------------------------
// Effect-body spawns — the data-loading shape.
// ---------------------------------------------------------------------

/// Mount a screen whose scope owns an effect reading `tick`, and run
/// `on_run` from inside that effect's body on every run. This is the
/// standard reload-counter data-loading shape: `effect!({ let _ =
/// tick.get(); spawn_then(fetch(), move |r| rows.set(r)); })`.
fn mount_effect_screen(
    h: &Harness,
    tick: Signal<i32>,
    on_run: impl Fn(Signal<i32>, i32) + 'static,
) -> Screen {
    let hole: Rc<RefCell<Option<Signal<bool>>>> = Rc::new(RefCell::new(None));
    let scoped: Rc<RefCell<Option<Signal<i32>>>> = Rc::new(RefCell::new(None));
    let hole_b = hole.clone();
    let scoped_b = scoped.clone();
    let on_run = Rc::new(on_run);
    let realized = h.mount(h.world.enter(|| {
        let shown = signal(true);
        *hole_b.borrow_mut() = Some(shown);
        view()
            .child(move || {
                if shown.get() {
                    let s = signal(0i32);
                    *scoped_b.borrow_mut() = Some(s);
                    let on_run = on_run.clone();
                    // Owned by the SCREEN's scope, so unmounting the screen
                    // frees this effect's slot.
                    runtime_world::effect(move || on_run(s, tick.get()));
                    view().build()
                } else {
                    view().build()
                }
            })
            .build()
    }));
    let shown = hole.borrow().expect("hole built");
    Screen { _realized: realized, shown, scoped }
}

/// The reported shape: a reload counter is bumped and the app navigates
/// away in the same window. The effect re-runs during the flush, its
/// `spawn_then` goes out, the screen is disposed, and the response lands
/// into a scope that is gone.
///
/// Regression: an effect RE-RUN is neither a build (`run_effect` pushes no
/// collector) nor — under a host-driven flush — inside any guarded
/// callback, so `ScopeAlive::current()` fell through to a fresh
/// `on_owned_drop` anchor. With no ambient collector that keepalive is
/// WORLD-ROOT-owned, so the token never flipped and `spawn_then` guarded
/// nothing: the callback ran and its first `Signal::set` aborted with
/// `idealyst[stale-signal-handle]`. The effect's FIRST run was always
/// guarded correctly (a build has a collector), which is why this only
/// ever bit reloads.
#[test]
fn effect_rerun_spawned_task_dies_with_its_owner() {
    ensure_executor();
    let h = Harness::new();
    let gate = Gate::new();
    let g = gate.clone();
    let ran: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let ran_c = ran.clone();
    let tick = h.world.enter(|| signal(0i32));

    let screen = mount_effect_screen(&h, tick, move |scoped, n| {
        if n == 0 {
            return; // first run is a build — the case under test is the re-run
        }
        let g = g.clone();
        let ran = ran_c.clone();
        spawn_then(
            async move {
                g.await;
                7i32
            },
            move |v| {
                *ran.borrow_mut() = true;
                scoped.set(v); // would abort if this ran
            },
        );
    });
    h.world.flush();

    h.world.enter(|| tick.set(1)); // reload
    h.world.flush(); // the effect re-runs and spawns
    pump(); // request in flight

    screen.shown.set(false); // navigate away — the screen's scope is freed
    h.world.flush();

    gate.complete();
    pump(); // the response lands into a disposed scope
    h.world.flush();

    assert!(
        !*ran.borrow(),
        "a task spawned from an effect RE-RUN must die with the scope that \
         owns the effect",
    );
}

/// The inverse, and the reason the anchor is the effect's SLOT rather than
/// `on_cleanup`: an in-flight task must survive both its own effect
/// re-running and the ordinary case where nothing is torn down at all.
/// Anchoring via `on_cleanup` (what the scheduling helpers use for timers)
/// would silently drop the result of every superseded fetch.
#[test]
fn effect_rerun_spawned_task_applies_while_its_owner_lives() {
    ensure_executor();
    let h = Harness::new();
    let gate = Gate::new();
    let g = gate.clone();
    let tick = h.world.enter(|| signal(0i32));

    let screen = mount_effect_screen(&h, tick, move |scoped, n| {
        if n != 1 {
            return; // exactly one task, issued by the first re-run
        }
        let g = g.clone();
        spawn_then(
            async move {
                g.await;
                7i32
            },
            move |v| scoped.set(v),
        );
    });
    h.world.flush();
    let scoped = screen.scoped.borrow().expect("mounted");

    h.world.enter(|| tick.set(1));
    h.world.flush();
    pump(); // in flight

    h.world.enter(|| tick.set(2)); // the same effect re-runs underneath it
    h.world.flush();

    gate.complete();
    pump();
    h.world.flush();

    assert_eq!(
        scoped.get(),
        7,
        "the screen is still mounted — a re-run of the spawning effect must \
         not cancel a request that already went out",
    );
}
