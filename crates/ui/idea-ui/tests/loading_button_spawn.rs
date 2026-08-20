//! Regression: the "busy button" — press → `busy.set(true)` →
//! `spawn_then(io, done)`, with `loading = busy` on the SAME Button —
//! must still run `done` when the IO resolves.
//!
//! A live structural prop routes the Button through its structure
//! `switch`, which rebuilds the pressable subtree when the tuple
//! changes. The press handler flips `busy`, so the very node that
//! mounted the handler is torn down by the handler's own write — and a
//! `spawn_then` reached from that handler anchored (via
//! `ScopeAlive::current`) to that node. Its liveness token flipped
//! before the IO came back, the callback was silently skipped, and the
//! app froze in its busy state: spinner forever, `busy` never reset,
//! result writes gone. Every save/submit button in a real app has this
//! exact shape, so the pattern broke app-wide the moment `spawn_then`
//! gained real handler-time anchoring (1.3.13).
//!
//! The fix re-anchors the Button's `on_click` to the component's own
//! scope (`Button` publishes its body-scope token around the call), so
//! a handler-reached spawn survives arm rebuilds but still dies with
//! the Button. Both halves are pinned here.
//!
//! Own integration test (own process) because it installs a buffering
//! test executor through the global first-install-wins
//! `runtime_shared::driver::install_async_executor` slot — the same
//! isolation rationale as runtime-vocabulary's `tests/scoped_spawn.rs`,
//! whose executor/Gate scaffolding this file repeats.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use host_mock::Harness;
use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui::components::button::Button;
use runtime_core::{signal, spawn_then, ui, Element, IntoElement, Signal};
use runtime_shared::driver::{install_async_executor, AsyncExecutor};

// ---------------------------------------------------------------------
// Test executor — buffering, thread-local, manually pumped.
// ---------------------------------------------------------------------

thread_local! {
    static TASKS: RefCell<Vec<Pin<Box<dyn Future<Output = ()> + 'static>>>> =
        const { RefCell::new(Vec::new()) };
}

struct TestExecutor;
// SAFETY: zero-sized; all live state is thread-local (each test thread
// pumps only its own queue) — the precedent in runtime-vocabulary's
// tests/async_reactive.rs.
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
// Harness — a busy Button, optionally behind a structural hole.
// ---------------------------------------------------------------------

struct Mounted {
    _realized: runtime_scene::Realized<host_mock::Node>,
    /// Collapsing the hole unmounts the Button; the world survives.
    shown: Signal<bool>,
    busy: Signal<bool>,
    result: Signal<i32>,
}

/// Mount `Button(loading = busy)` whose handler does the app-standard
/// busy dance: guard, flip `busy`, spawn the IO, reset in the callback.
/// `ran` flips when the callback runs — the assertion handle that stays
/// valid even after the Button's own signals are freed by an unmount.
fn mount_busy_button(h: &Harness, gate: Gate, ran: Rc<RefCell<bool>>) -> Mounted {
    let hole: Rc<RefCell<Option<Signal<bool>>>> = Rc::new(RefCell::new(None));
    let cell: Rc<RefCell<Option<(Signal<bool>, Signal<i32>)>>> = Rc::new(RefCell::new(None));
    let hole_b = hole.clone();
    let cell_b = cell.clone();
    let realized = h.mount(h.world.enter(|| {
        install_idea_theme(light_theme());
        let shown = signal(true);
        *hole_b.borrow_mut() = Some(shown);
        runtime_core::view(vec![runtime_core::dynamic(move || -> Element {
            if !shown.get() {
                return runtime_core::view(Vec::new()).into_element();
            }
            let busy = signal(false);
            let result = signal(0i32);
            *cell_b.borrow_mut() = Some((busy, result));
            let gate = gate.clone();
            let ran = ran.clone();
            let on_click: Rc<dyn Fn()> = Rc::new(move || {
                if busy.get() {
                    return;
                }
                busy.set(true);
                let gate = gate.clone();
                let ran = ran.clone();
                spawn_then(
                    async move {
                        gate.await;
                        7i32
                    },
                    move |v| {
                        *ran.borrow_mut() = true;
                        result.set(v); // would abort if this ran after unmount
                        busy.set(false);
                    },
                );
            });
            ui! {
                Button(label = "Save", on_click = on_click.clone(), loading = busy)
            }
            .into_element()
        })])
        .into_element()
    }));
    h.flush();
    let shown = hole.borrow().expect("hole built");
    let (busy, result) = cell.borrow().expect("button built");
    Mounted { _realized: realized, shown, busy, result }
}

// ---------------------------------------------------------------------

/// The regression: the handler's own `busy` flip rebuilds the pressable
/// arm, and the pending callback must survive that rebuild.
#[test]
fn busy_button_callback_survives_its_own_loading_flip() {
    ensure_executor();
    let h = Harness::new();
    let gate = Gate::new();
    let ran: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let m = mount_busy_button(&h, gate.clone(), ran.clone());

    h.press_handler(0)(); // user taps Save
    h.world.flush(); // busy flips → the structure `switch` rebuilds the arm
    assert!(m.busy.get(), "the press must enter the busy state");
    pump(); // request in flight

    gate.complete();
    pump();
    h.world.flush();

    assert!(*ran.borrow(), "the done-callback must run after the IO resolves");
    assert_eq!(m.result.get(), 7, "the done-callback must apply its result");
    assert!(!m.busy.get(), "busy must reset — a stuck spinner is the regression");
}

/// The guarantee the fix must NOT loosen: unmount the whole Button while
/// the IO is in flight and the callback still never runs.
#[test]
fn busy_button_callback_still_dies_with_the_button() {
    ensure_executor();
    let h = Harness::new();
    let gate = Gate::new();
    let ran: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let m = mount_busy_button(&h, gate.clone(), ran.clone());

    h.press_handler(0)();
    h.world.flush();
    pump(); // request in flight

    m.shown.set(false); // navigate away — the Button's scope is freed
    h.world.flush();

    gate.complete();
    pump();
    h.world.flush();

    assert!(
        !*ran.borrow(),
        "a task spawned from the Button's handler must die with the Button"
    );
}
