//! `spawn-anchor-repro` — a press that closes its own overlay still gets
//! the reply to the work it started.
//!
//! Each card is one shape of "press, start a request, show a spinner until
//! the reply lands". The request is a 1.5 s timer standing in for a server
//! call. Every card counts the requests it sent and the replies it applied.
//!
//! - **Confirm in a dialog**: the button that starts the work is inside a
//!   `Modal`, and its press closes the `Modal` (CrewForge's "Hand in empty
//!   sheet"). The control is gone when the reply lands; the reply still
//!   runs, because `Modal` marks itself an overlay (`mark_overlay`) and the
//!   card that opened it is still mounted.
//! - **Pick from a menu**: the same with a `MenuItem` in a `Menu`.
//! - **Confirm in a dialog, anchored to the card**: the dialog case with
//!   the reply tied to the card by hand, `ScopeAlive::current()` in the
//!   card's body and `spawn_then_in`. Same result.
//! - **Save, then cancel while it saves**: the dialog stays open while the
//!   request runs and writes its own error line when the reply lands. Press
//!   Cancel before then. The reply runs; its write to the closed dialog's
//!   line lands on state kept for it and shows nowhere.
//!
//! Without `mark_overlay` the first two cards lost their replies: the
//! counts stayed one apart and the spinner never stopped.

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;
use std::task::{Poll, Waker};

use idea_ui::{
    install_idea_theme, light_theme, tone, typography_kind, variant, Button, Menu, MenuItem, Modal, Typography,
};
use runtime_core::{
    after_ms_detached, component, signal, spawn_then, spawn_then_in, stylesheet, ui, AlignItems, AnchorTarget,
    Element, FlexDirection, FlexWrap, PressableHandle, Ref, SafeAreaSides, ScopeAlive, Signal,
};

/// How long the stand-in request takes.
const REQUEST_MS: i32 = 1500;

/// A future that resolves after `ms`: the request every card makes. It
/// touches no signal, as `spawn_then`'s future must not.
fn request(ms: i32) -> impl Future<Output = ()> {
    let state: Rc<RefCell<(bool, Option<Waker>)>> = Rc::new(RefCell::new((false, None)));
    let fire = state.clone();
    after_ms_detached(ms, move || {
        let mut s = fire.borrow_mut();
        s.0 = true;
        if let Some(w) = s.1.take() {
            w.wake();
        }
    });
    std::future::poll_fn(move |cx| {
        let mut s = state.borrow_mut();
        if s.0 {
            Poll::Ready(())
        } else {
            s.1 = Some(cx.waker().clone());
            Poll::Pending
        }
    })
}

stylesheet! {
    pub Screen<()> {
        base(_t) {
            flex_grow: 1.0,
            padding: 24,
            gap: 20,
        }
    }
}

stylesheet! {
    pub Cards<()> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Wrap,
            gap: 16,
        }
    }
}

stylesheet! {
    pub CardBox<()> {
        base(_t) {
            width: 300,
            padding: 16,
            gap: 10,
            border_width: 1.0,
            border_radius: 10,
            align_items: AlignItems::FlexStart,
        }
    }
}

/// What every card shows: a title, what to do, and the two counts.
#[component]
fn Card(
    #[prop(static)] title: String,
    #[prop(static)] steps: String,
    sent: Signal<u32>,
    applied: Signal<u32>,
    children: Vec<Element>,
) -> Element {
    ui! {
        view(style = CardBox()) {
            Typography(content = title, kind = typography_kind::H3)
            Typography(content = steps, muted = true)
            children
            text { move || format!("requests sent: {}", sent.get()) }
            text { move || format!("replies applied: {}", applied.get()) }
        }
    }
}

/// The button that opens the case, spinning while its request is out.
fn opener(label: &str, busy: Signal<bool>, on_click: Rc<dyn Fn()>, bind: Option<Ref<PressableHandle>>) -> Element {
    ui! {
        Button(
            label = label.to_string(),
            loading = busy,
            tone = tone::Neutral,
            variant = variant::Soft,
            on_click = on_click,
            bind_to = bind,
        )
    }
}

/// The CrewForge shape: the dialog's own button closes the dialog and
/// starts the work.
#[component]
fn DialogConfirm() -> Element {
    let (open, busy, sent, applied) = (signal(false), signal(false), signal(0u32), signal(0u32));
    let show: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    let confirm: Rc<dyn Fn()> = Rc::new(move || {
        open.set(false);
        busy.set(true);
        sent.update(|n| n + 1);
        spawn_then(request(REQUEST_MS), move |()| {
            busy.set(false);
            applied.update(|n| n + 1);
        });
    });
    ui! {
        Card(
            title = "Confirm in a dialog".to_string(),
            steps = "Press Hand in, then Hand in empty sheet.".to_string(),
            sent = sent,
            applied = applied,
        ) {
            { opener("Hand in", busy, show, None) }
            Modal(
                open = open,
                on_dismiss = Some(close.clone()),
                content = move || ui! {
                    Typography(content = "Nothing is filed for this day.")
                },
                footer = move || ui! {
                    Button(label = "Hand in empty sheet", on_click = confirm.clone())
                },
            )
        }
    }
}

/// The same with a menu: the row's press closes the menu.
#[component]
fn MenuPick() -> Element {
    let (open, busy, sent, applied) = (signal(false), signal(false), signal(0u32), signal(0u32));
    let trigger: Ref<PressableHandle> = Ref::new();
    let show: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    let pick: Rc<dyn Fn()> = Rc::new(move || {
        open.set(false);
        busy.set(true);
        sent.update(|n| n + 1);
        spawn_then(request(REQUEST_MS), move |()| {
            busy.set(false);
            applied.update(|n| n + 1);
        });
    });
    ui! {
        Card(
            title = "Pick from a menu".to_string(),
            steps = "Press Status, then Hand in.".to_string(),
            sent = sent,
            applied = applied,
        ) {
            { opener("Status", busy, show, Some(trigger)) }
            if open.get() {
                Menu(target = Some(AnchorTarget::from(trigger)), on_dismiss = Some(close.clone())) {
                    MenuItem(label = "Hand in", on_select = pick.clone())
                }
            }
        }
    }
}

/// The dialog case with today's remedy: the reply is tied to this card,
/// which outlives the dialog.
#[component]
fn DialogConfirmAnchored() -> Element {
    let (open, busy, sent, applied) = (signal(false), signal(false), signal(0u32), signal(0u32));
    let alive = ScopeAlive::current();
    let show: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let close: Rc<dyn Fn()> = Rc::new(move || open.set(false));
    let confirm: Rc<dyn Fn()> = Rc::new(move || {
        open.set(false);
        busy.set(true);
        sent.update(|n| n + 1);
        spawn_then_in(&alive, request(REQUEST_MS), move |()| {
            busy.set(false);
            applied.update(|n| n + 1);
        });
    });
    ui! {
        Card(
            title = "Confirm in a dialog, anchored to the card".to_string(),
            steps = "Press Hand in, then Hand in empty sheet.".to_string(),
            sent = sent,
            applied = applied,
        ) {
            { opener("Hand in", busy, show, None) }
            Modal(
                open = open,
                on_dismiss = Some(close.clone()),
                content = move || ui! {
                    Typography(content = "Nothing is filed for this day.")
                },
                footer = move || ui! {
                    Button(label = "Hand in empty sheet", on_click = confirm.clone())
                },
            )
        }
    }
}

/// The case a fix must not break: the dialog owns the line the reply
/// writes, and the reader closes the dialog before the reply lands.
#[component]
fn SaveThenCancel() -> Element {
    let (open, busy, sent, applied) = (signal(false), signal(false), signal(0u32), signal(0u32));
    let show: Rc<dyn Fn()> = Rc::new(move || open.set(true));
    let close: Rc<dyn Fn()> = Rc::new(move || {
        open.set(false);
        busy.set(false);
    });
    let cancel = close.clone();
    ui! {
        Card(
            title = "Save, then cancel while it saves".to_string(),
            steps = "Press Edit, then Save, then Cancel within 1.5 s.".to_string(),
            sent = sent,
            applied = applied,
        ) {
            { opener("Edit", busy, show, None) }
            Modal(
                open = open,
                on_dismiss = Some(close.clone()),
                content = move || {
                    // The dialog's own state: created with it, gone with it.
                    let error: Signal<Option<String>> = signal(None);
                    let save: Rc<dyn Fn()> = Rc::new(move || {
                        busy.set(true);
                        sent.update(|n| n + 1);
                        spawn_then(request(REQUEST_MS), move |()| {
                            busy.set(false);
                            applied.update(|n| n + 1);
                            error.set(Some("The server refused the save.".to_string()));
                        });
                    });
                    let cancel = cancel.clone();
                    ui! {
                        Typography(content = "Edit the record.")
                        text { move || error.get().unwrap_or_default() }
                        view {
                            Button(label = "Save", loading = busy, on_click = save)
                            Button(label = "Cancel", variant = variant::Outlined, on_click = cancel)
                        }
                    }
                },
            )
        }
    }
}

pub fn app() -> Element {
    install_idea_theme(light_theme());
    ui! {
        view(style = Screen(), safe_area = SafeAreaSides::TOP) {
            Typography(content = "A press that closes its own overlay keeps its reply", kind = typography_kind::H2)
            Typography(
                content = "Each request takes 1.5 s. Every card should end with as many replies applied as requests sent, its spinner stopped.",
                muted = true,
            )
            view(style = Cards()) {
                DialogConfirm()
                MenuPick()
                DialogConfirmAnchored()
                SaveThenCancel()
            }
        }
    }
}

/// The Android wrapper's entry (see `examples/README.md`).
pub fn scene_app() -> Element {
    app()
}

/// SDK-handler registration seam. This app registers nothing.
pub fn register_scene_extensions<H: runtime_scene::Host>(_registry: &mut runtime_scene::Registry<H>) {}
