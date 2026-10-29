//! Regression: a picker's commit handler (`Select` / `Autocomplete` /
//! `DatePicker` `on_change`) that does the app-standard busy dance —
//! `busy.set(true)` → `spawn_then(io, done)` — must still run `done`.
//!
//! The commit fires from a row inside the picker's popup, and the commit
//! closes that popup. A `spawn_then` reached from the handler anchored
//! (via `ScopeAlive::current`) to the row that mounted the handler, which
//! the close tears down, so the callback was silently skipped: the request
//! landed, its answer was discarded, and `busy` stayed set. The pickers
//! now re-anchor their commit to their own component scope
//! (`components::anchor_to_component`), so the callback survives the
//! popup closing and still dies with the picker.
//!
//! Own integration test (own process) because it installs a buffering
//! test executor through the global first-install-wins
//! `install_async_executor` slot — the scaffolding of
//! `tests/loading_button_spawn.rs`.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use host_mock::Harness;
use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui::{Autocomplete, Select, SelectOption};
use runtime_core::{signal, spawn_then, ui, Element, IntoElement, Signal};
use runtime_shared::driver::{install_async_executor, AsyncExecutor};

thread_local! {
    static TASKS: RefCell<Vec<Pin<Box<dyn Future<Output = ()> + 'static>>>> =
        const { RefCell::new(Vec::new()) };
}

struct TestExecutor;
// SAFETY: zero-sized; all live state is thread-local (each test thread
// pumps only its own queue).
unsafe impl Send for TestExecutor {}
unsafe impl Sync for TestExecutor {}

impl AsyncExecutor for TestExecutor {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
        TASKS.with(|t| t.borrow_mut().push(future));
    }
}

fn pump() {
    let mut tasks = TASKS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    let mut cx = Context::from_waker(std::task::Waker::noop());
    tasks.retain_mut(|f| f.as_mut().poll(&mut cx).is_pending());
    TASKS.with(|t| {
        let mut q = t.borrow_mut();
        tasks.append(&mut q);
        *q = tasks;
    });
}

#[derive(Clone)]
struct Gate(Rc<RefCell<bool>>);
impl Future for Gate {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if *self.0.borrow() { Poll::Ready(()) } else { Poll::Pending }
    }
}

#[derive(Clone, Copy)]
enum Picker {
    Select,
    Autocomplete,
}

struct Mounted {
    _realized: runtime_scene::Realized<host_mock::Node>,
    /// Collapsing the hole unmounts the picker; the world survives.
    shown: Signal<bool>,
    /// The id the commit handler last received.
    picked: Rc<RefCell<Option<String>>>,
}

/// Mount `picker` behind a structural hole, with a commit handler that
/// flips `busy`, spawns IO gated on `gate`, and records `ran` when the
/// done-callback runs.
fn mount(h: &Harness, picker: Picker, gate: Gate, ran: Rc<RefCell<bool>>) -> Mounted {
    let hole: Rc<RefCell<Option<Signal<bool>>>> = Rc::new(RefCell::new(None));
    let hole_b = hole.clone();
    let picked: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let picked_b = picked.clone();
    let realized = h.mount(h.world.enter(|| {
        install_idea_theme(light_theme());
        let shown = signal(true);
        *hole_b.borrow_mut() = Some(shown);
        runtime_core::view(vec![runtime_core::dynamic(move || -> Element {
            if !shown.get() {
                return runtime_core::view(Vec::new()).into_element();
            }
            let value = signal(String::new());
            let busy = signal(false);
            let (gate, ran) = (gate.clone(), ran.clone());
            let picked = picked_b.clone();
            let on_change: Rc<dyn Fn(String)> = Rc::new(move |id: String| {
                *picked.borrow_mut() = Some(id.clone());
                value.set(id);
                busy.set(true);
                let (gate, ran) = (gate.clone(), ran.clone());
                spawn_then(gate, move |()| {
                    *ran.borrow_mut() = true;
                    busy.set(false); // would abort if this ran after unmount
                });
            });
            match picker {
                Picker::Select => ui! {
                    Select(
                        value = value,
                        options = vec![SelectOption::new("a", "Apple"), SelectOption::new("b", "Pear")],
                        on_change = on_change.clone(),
                    )
                },
                Picker::Autocomplete => ui! {
                    Autocomplete(
                        value = value,
                        options = vec![SelectOption::new("a", "Apple"), SelectOption::new("b", "Pear")],
                        on_change = on_change.clone(),
                    )
                },
            }
        })])
        .into_element()
    }));
    h.flush();
    let shown = hole.borrow().expect("hole built");
    Mounted { _realized: realized, shown, picked }
}

/// Open the picker's popup and press its "Apple" row.
fn pick_first(h: &Harness, m: &Mounted, picker: Picker) {
    match picker {
        Picker::Select => h.press_handler(0)(),
        Picker::Autocomplete => (h.shared.focus_handlers.borrow()[0].1)(true),
    }
    h.world.flush();
    h.press_labelled("Apple");
    h.world.flush();
    assert_eq!(m.picked.borrow().as_deref(), Some("a"), "the press committed the option");
}

fn commit_survives_the_popup_closing(picker: Picker) {
    install_async_executor(Box::new(TestExecutor));
    let h = Harness::new();
    let gate = Gate(Rc::new(RefCell::new(false)));
    let ran = Rc::new(RefCell::new(false));
    let m = mount(&h, picker, gate.clone(), ran.clone());

    pick_first(&h, &m, picker); // the pick closes the popup
    pump(); // request in flight
    *gate.0.borrow_mut() = true;
    pump();
    h.world.flush();

    assert!(*ran.borrow(), "the done-callback must run after the popup that fired it closed");
}

fn commit_still_dies_with_the_picker(picker: Picker) {
    install_async_executor(Box::new(TestExecutor));
    let h = Harness::new();
    let gate = Gate(Rc::new(RefCell::new(false)));
    let ran = Rc::new(RefCell::new(false));
    let m = mount(&h, picker, gate.clone(), ran.clone());

    pick_first(&h, &m, picker);
    pump();
    m.shown.set(false); // navigate away — the picker's scope is freed
    h.world.flush();
    *gate.0.borrow_mut() = true;
    pump();
    h.world.flush();

    assert!(!*ran.borrow(), "a task spawned from the commit must die with the picker");
}

#[test]
fn regression_select_on_change_spawn_survives_the_menu_closing() {
    commit_survives_the_popup_closing(Picker::Select);
}

#[test]
fn select_on_change_spawn_dies_with_the_select() {
    commit_still_dies_with_the_picker(Picker::Select);
}

#[test]
fn regression_autocomplete_on_change_spawn_survives_the_menu_closing() {
    commit_survives_the_popup_closing(Picker::Autocomplete);
}

#[test]
fn autocomplete_on_change_spawn_dies_with_the_autocomplete() {
    commit_still_dies_with_the_picker(Picker::Autocomplete);
}
