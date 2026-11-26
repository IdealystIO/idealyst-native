//! A press inside an overlay that the press itself closes, starting work
//! whose reply clears a busy flag: the shape of a confirm dialog's
//! "Hand in" and of a status menu's move. The four cases of
//! `examples/spawn-anchor-repro`, pinned:
//!
//! - a dialog's button closes the dialog and starts the work — the reply
//!   must land;
//! - the same from a menu row;
//! - the dialog case anchored to the card by hand (`spawn_then_in`) —
//!   the reply lands today;
//! - a dialog that owns the line its reply writes, closed by the reader
//!   while the work is out — the reply must not write to the closed
//!   dialog, and nothing may panic;
//! - CrewForge's shape: a `match` arm holding a dialog, which the
//!   handler's own write removes;
//! - a popover;
//! - a dialog whose opener is itself removed while the work is out — the
//!   reply is dropped, as for any control that left.
//!
//! Own integration test (own process): it installs a buffering test
//! executor through the global first-install-wins slot, as
//! `loading_button_spawn.rs` does.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use host_mock::Harness;
use idea_theme::theme::{install_idea_theme, light_theme};
use idea_ui::components::button::Button;
use idea_ui::components::menu::{Menu, MenuItem};
use idea_ui::components::modal::Modal;
use idea_ui::components::popover::Popover;
use runtime_core::{
    signal, spawn_then, spawn_then_in, text, ui, AnchorTarget, Element, PressableHandle, Ref, ScopeAlive, Signal,
};
use runtime_shared::driver::{install_async_executor, AsyncExecutor};

thread_local! {
    static TASKS: RefCell<Vec<Pin<Box<dyn Future<Output = ()> + 'static>>>> =
        const { RefCell::new(Vec::new()) };
}

struct TestExecutor;
// SAFETY: zero-sized; all live state is thread-local (each test thread
// pumps only its own queue), as in `loading_button_spawn.rs`.
unsafe impl Send for TestExecutor {}
unsafe impl Sync for TestExecutor {}

impl AsyncExecutor for TestExecutor {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
        TASKS.with(|t| t.borrow_mut().push(future));
    }
}

/// Poll every queued future once.
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

/// The request: in flight until the test completes it.
#[derive(Clone)]
struct Gate(Rc<RefCell<bool>>);
impl Future for Gate {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if *self.0.borrow() { Poll::Ready(()) } else { Poll::Pending }
    }
}

/// What a case hands back: the card's busy flag and whether the reply
/// ran.
struct Case {
    h: Harness,
    _realized: runtime_scene::Realized<host_mock::Node>,
    busy: Signal<bool>,
    ran: Rc<RefCell<bool>>,
    gate: Gate,
}

fn mount(build: impl FnOnce(Gate, Rc<RefCell<bool>>) -> (Element, Signal<bool>)) -> Case {
    install_async_executor(Box::new(TestExecutor));
    host_mock::pump::install_scheduler();
    let h = Harness::new();
    let gate = Gate(Rc::new(RefCell::new(false)));
    let ran = Rc::new(RefCell::new(false));
    let (tree, busy) = h.world.enter(|| {
        install_idea_theme(light_theme());
        build(gate.clone(), ran.clone())
    });
    let realized = h.mount(tree);
    h.flush();
    host_mock::pump::pump_frames();
    h.flush();
    Case { h, _realized: realized, busy, ran, gate }
}

impl Case {
    fn press(&self, label: &str) {
        self.h.press_labelled(label);
        self.h.flush();
        host_mock::pump::pump_timers();
        self.h.flush();
        pump();
    }

    /// The reply lands.
    fn reply(&self) {
        *self.gate.0.borrow_mut() = true;
        pump();
        self.h.flush();
    }
}

/// The handler every case shares: close the overlay, go busy, start the
/// request, and clear busy when it answers.
fn hand_in(open: Signal<bool>, busy: Signal<bool>, gate: Gate, ran: Rc<RefCell<bool>>) -> Rc<dyn Fn()> {
    Rc::new(move || {
        open.set(false);
        busy.set(true);
        let (gate, ran) = (gate.clone(), ran.clone());
        spawn_then(gate, move |()| {
            *ran.borrow_mut() = true;
            busy.set(false);
        });
    })
}

#[test]
fn a_dialogs_button_that_closes_the_dialog_gets_its_reply() {
    let case = mount(|gate, ran| {
        let (open, busy) = (signal(true), signal(false));
        let confirm = hand_in(open, busy, gate, ran);
        let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
        let tree = ui! {
            view {
                Button(label = "Hand in", loading = busy, on_click = Rc::new(move || open.set(true)) as Rc<dyn Fn()>)
                Modal(
                    open = open,
                    on_dismiss = Some(close.clone()),
                    content = move || ui! { Button(label = "Hand in empty sheet", on_click = confirm.clone()) },
                )
            }
        };
        (tree, busy)
    });
    case.press("Hand in empty sheet");
    assert!(case.h.world.enter(|| case.busy.get()), "the press went busy");
    case.reply();
    assert!(*case.ran.borrow(), "the reply ran");
    assert!(!case.h.world.enter(|| case.busy.get()), "the spinner stopped");
}

#[test]
fn a_menu_row_that_closes_the_menu_gets_its_reply() {
    let case = mount(|gate, ran| {
        let (open, busy) = (signal(true), signal(false));
        let trigger: Ref<PressableHandle> = Ref::new();
        let pick = hand_in(open, busy, gate, ran);
        let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
        let tree = ui! {
            view {
                Button(label = "Status", loading = busy, on_click = Rc::new(move || open.set(true)) as Rc<dyn Fn()>, bind_to = Some(trigger))
                if open.get() {
                    Menu(target = Some(AnchorTarget::from(trigger)), on_dismiss = Some(close.clone())) {
                        MenuItem(label = "Hand in", on_select = pick.clone())
                    }
                }
            }
        };
        (tree, busy)
    });
    case.press("Hand in");
    assert!(case.h.world.enter(|| case.busy.get()), "the press went busy");
    case.reply();
    assert!(*case.ran.borrow(), "the reply ran");
    assert!(!case.h.world.enter(|| case.busy.get()), "the spinner stopped");
}

#[test]
fn a_dialogs_button_anchored_to_its_card_by_hand_gets_its_reply() {
    let case = mount(|gate, ran| {
        let (open, busy) = (signal(true), signal(false));
        let alive = ScopeAlive::current();
        let confirm: Rc<dyn Fn()> = Rc::new(move || {
            open.set(false);
            busy.set(true);
            let (gate, ran) = (gate.clone(), ran.clone());
            spawn_then_in(&alive, gate, move |()| {
                *ran.borrow_mut() = true;
                busy.set(false);
            });
        });
        let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
        let tree = ui! {
            view {
                Button(label = "Hand in", loading = busy, on_click = Rc::new(move || open.set(true)) as Rc<dyn Fn()>)
                Modal(
                    open = open,
                    on_dismiss = Some(close.clone()),
                    content = move || ui! { Button(label = "Hand in empty sheet", on_click = confirm.clone()) },
                )
            }
        };
        (tree, busy)
    });
    case.press("Hand in empty sheet");
    case.reply();
    assert!(*case.ran.borrow(), "the reply ran");
    assert!(!case.h.world.enter(|| case.busy.get()), "the spinner stopped");
}

/// The dialog owns the line its reply writes; the reader closes the
/// dialog before the reply. The write must not land on the closed
/// dialog's state, and nothing may panic.
#[test]
fn a_reply_to_a_dialog_closed_while_it_saved_does_not_touch_the_dialog() {
    let case = mount(|gate, ran| {
        let (open, busy) = (signal(true), signal(false));
        let close: Rc<dyn Fn()> = Rc::new(move || {
            open.set(false);
            busy.set(false);
        });
        let cancel = close.clone();
        let tree = ui! {
            view {
                Button(label = "Edit", loading = busy, on_click = Rc::new(move || open.set(true)) as Rc<dyn Fn()>)
                Modal(
                    open = open,
                    on_dismiss = Some(close.clone()),
                    content = move || {
                        let error: Signal<Option<String>> = signal(None);
                        let (gate, ran) = (gate.clone(), ran.clone());
                        let save: Rc<dyn Fn()> = Rc::new(move || {
                            busy.set(true);
                            let (gate, ran) = (gate.clone(), ran.clone());
                            spawn_then(gate, move |()| {
                                *ran.borrow_mut() = true;
                                busy.set(false);
                                error.set(Some("refused".to_string()));
                            });
                        });
                        let cancel = cancel.clone();
                        ui! {
                            text { move || error.get().unwrap_or_default() }
                            Button(label = "Save", on_click = save)
                            Button(label = "Cancel", on_click = cancel)
                        }
                    },
                )
            }
        };
        (tree, busy)
    });
    case.press("Save");
    case.press("Cancel");
    case.reply();
    assert!(!case.h.world.enter(|| case.busy.get()), "Cancel cleared the spinner");
    let screen = case
        .h
        .live_roots()
        .into_iter()
        .map(|r| case.h.live_tree(r))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!screen.contains("refused"), "the closed dialog's line never shows:\n{screen}");
}

/// CrewForge's "Hand in empty sheet": the dialog is a `match` arm, and the
/// handler's `gate.set(None)` removes the whole arm.
#[test]
fn regression_a_dialog_arm_the_handler_removes_gets_its_reply() {
    let case = mount(|gate_io, ran| {
        let (gate, busy) = (signal(Some(1u8)), signal(false));
        let close: Rc<dyn Fn()> = Rc::new(move || gate.set(None));
        let confirm: Rc<dyn Fn()> = Rc::new(move || {
            gate.set(None);
            busy.set(true);
            let (gate_io, ran) = (gate_io.clone(), ran.clone());
            spawn_then(gate_io, move |()| {
                *ran.borrow_mut() = true;
                busy.set(false);
            });
        });
        let tree = ui! {
            view {
                match gate.get() {
                    Some(_) => {
                        Modal(
                            open = true,
                            on_dismiss = Some(close.clone()),
                            content = { let confirm = confirm.clone(); move || ui! { Button(label = "Hand in empty sheet", on_click = confirm.clone()) } },
                        )
                    }
                    None => { view {} }
                }
            }
        };
        (tree, busy)
    });
    case.press("Hand in empty sheet");
    case.reply();
    assert!(*case.ran.borrow(), "the reply ran");
    assert!(!case.h.world.enter(|| case.busy.get()), "the spinner stopped");
}

#[test]
fn a_popover_button_that_closes_the_popover_gets_its_reply() {
    let case = mount(|gate, ran| {
        let (open, busy) = (signal(true), signal(false));
        let anchor: Ref<runtime_core::ViewHandle> = Ref::new();
        let go = hand_in(open, busy, gate, ran);
        let tree = ui! {
            view {
                view(bind = anchor) { text { "trigger" } }
                if open.get() {
                    Popover(target = Some(AnchorTarget::from(anchor)), sheet_on_phone = false) {
                        Button(label = "Hand in", on_click = go.clone())
                    }
                }
            }
        };
        (tree, busy)
    });
    case.press("Hand in");
    case.reply();
    assert!(*case.ran.borrow(), "the reply ran");
    assert!(!case.h.world.enter(|| case.busy.get()), "the spinner stopped");
}

/// The dialog's opener goes too while the work is out (its card is
/// removed): the reply is dropped, as for any control that left.
#[test]
fn a_reply_whose_dialogs_opener_also_left_is_dropped() {
    let shown_cell: Rc<RefCell<Option<Signal<bool>>>> = Rc::new(RefCell::new(None));
    let cell = shown_cell.clone();
    let case = mount(move |gate, ran| {
        let shown = signal(true);
        *cell.borrow_mut() = Some(shown);
        let busy = signal(false);
        let tree = ui! {
            view {
                if shown.get() {
                    {
                        let open = signal(true);
                        let confirm = hand_in(open, busy, gate.clone(), ran.clone());
                        ui! {
                            view {
                                if open.get() {
                                    Modal(
                                        open = true,
                                        content = { let confirm = confirm.clone(); move || ui! { Button(label = "Hand in empty sheet", on_click = confirm.clone()) } },
                                    )
                                }
                            }
                        }
                    }
                }
            }
        };
        (tree, busy)
    });
    case.press("Hand in empty sheet");
    let shown = shown_cell.borrow().expect("built");
    case.h.world.enter(|| shown.set(false));
    case.h.flush();
    host_mock::pump::pump_timers();
    case.h.flush();
    case.reply();
    assert!(!*case.ran.borrow(), "the opener left too: the reply is dropped");
}
