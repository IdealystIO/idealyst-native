//! Idealyst Inspector — a runtime debugging dashboard.
//!
//! Connects to a running idealyst app's **robot bridge** (the single TCP
//! newline-JSON transport every `--features robot` app exposes) and shows,
//! live: every loaded navigator (current + back-stack), the element tree,
//! components and their callable methods, reactive-arena + perf phase
//! counters, watched signal values, and captured logs. A small command bar
//! drives the target back (click an element by `test_id`, invoke a
//! component method, clear logs).
//!
//! ## Run it
//!
//! ```text
//! # 1. a target app, built with the robot bridge:
//! idealyst dev --macos --local examples/conformance
//! # 2. the inspector (this app), in another shell:
//! idealyst dev --macos --local examples/inspector
//! ```
//!
//! MVP host is macOS desktop: raw TCP to the bridge is trivial there. (Web
//! can't open raw TCP from wasm; that would need the bridge to also speak
//! WebSocket — a separate, single-transport change, deliberately out of
//! scope here.)

use std::cell::RefCell;
use std::rc::Rc;

use std::collections::HashSet;

use idea_ui::{install_idea_theme, light_theme, Button, Stack, StackGap, StackPadding, Tab, Tabs};
use runtime_core::{
    component, signal, text, ui, Element, IntoElement, Ref, Route, Screen, Signal,
};
use serde_json::Value;
use stack_navigator::{Navigator, StackBuilder, StackHandle, StackScreenExt};

mod client;
mod discovery;
mod format;
mod panels;

use client::{BridgeClient, Snapshot};
use panels::{ElementsPanel, LogsPanel, StatsPanel};

/// How often the UI thread copies the background client's latest snapshot
/// into the reactive signal. The client refreshes independently; this is
/// just the render cadence.
const POLL_MS: i32 = 300;

pub(crate) const PICKER: Route<()> = Route::<()>::new("picker", "/");
pub(crate) const INSPECTOR: Route<()> = Route::<()>::new("inspector", "/inspector");

thread_local! {
    /// The currently-connected target. Single-target at a time; the picker
    /// swaps it. Kept here (not as a component prop) so the command bar and
    /// poll reach it without threading a non-`Default` handle through props.
    static CURRENT_CLIENT: RefCell<Option<BridgeClient>> = const { RefCell::new(None) };
}

/// Connect to a discovered app, replacing any prior connection.
fn connect_to(addr: String) {
    CURRENT_CLIENT.with(|c| *c.borrow_mut() = Some(BridgeClient::connect(addr)));
}

/// The latest snapshot from the connected target (default = disconnected).
fn client_snapshot() -> Snapshot {
    CURRENT_CLIENT.with(|c| {
        c.borrow()
            .as_ref()
            .map(|cl| cl.snapshot())
            .unwrap_or_default()
    })
}

/// Queue an action verb for the connected target (no-op when disconnected).
fn client_action(cmd: &str, args: Value) {
    CURRENT_CLIENT.with(|c| {
        if let Some(cl) = c.borrow().as_ref() {
            cl.action(cmd, args);
        }
    });
}

pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[component]
pub fn app() -> Element {
    install_idea_theme(light_theme());

    // The reactive mirror of the target's state, refreshed by the poll below.
    let snapshot: Signal<Snapshot> = signal!(Snapshot::default());
    let nav: Ref<StackHandle> = Ref::new();

    // UI-thread poll: copy the background client's latest snapshot into the
    // signal so the reactive `text` panels re-render. Self-reschedules.
    schedule_poll(snapshot);

    let builder = Navigator::new(&PICKER)
        .screen(PICKER, move |_| {
            Screen::new(picker_page(nav)).title("Idealyst Inspector")
        })
        .screen(INSPECTOR, move |_| {
            Screen::new(inspector_page(snapshot, nav)).title("Inspector")
        });

    ui! { builder.bind(nav) }
}

fn schedule_poll(snapshot: Signal<Snapshot>) {
    runtime_core::after_ms_detached(POLL_MS, move || {
        // Never write a signal while the reactive system is mid-mutation —
        // a `set` during that window re-enters the arena borrow and panics.
        // If we land in that window, skip this tick and catch the next one.
        if !runtime_core::is_reactive_busy() {
            // Only push when the data actually changed. The panels are now
            // selectable `pressable` rows inside reactive `#[component]`
            // scopes; an unconditional `set` every tick would rebuild every
            // row 3×/s — wasted work, and a press could race a rebuild.
            // `set` notifies subscribers unconditionally, so we gate here.
            let next = client_snapshot();
            if snapshot.get() != next {
                snapshot.set(next);
            }
        }
        schedule_poll(snapshot);
    });
}

/// App picker — one button per discovered app. Tapping connects and opens
/// the inspector. Rescans by resetting the screen.
fn picker_page(nav: Ref<StackHandle>) -> Element {
    let apps = discovery::list();

    let mut children: Vec<Element> = Vec::new();
    children.push(text("Running idealyst apps").into_element());

    if apps.is_empty() {
        children.push(
            text(
                "No apps found in ~/.idealyst/apps. Launch one with \
                 `--features robot` (e.g. `idealyst dev --macos --local \
                 examples/conformance`), then Rescan.",
            )
            .into_element(),
        );
    } else {
        for app in apps {
            let label = format!(
                "{}{}  (pid {}, :{})",
                app.name,
                app.bundle_id
                    .as_deref()
                    .map(|b| format!("  [{b}]"))
                    .unwrap_or_default(),
                app.pid,
                app.port
            );
            let addr = app.addr();
            let nav = nav;
            let on_click: Rc<dyn Fn()> = Rc::new(move || {
                connect_to(addr.clone());
                // `.get()` (clone the handle out) NOT `.with()` — `with`
                // holds the arena borrow across the closure, and `push`
                // sets `active_route`, which re-enters `ARENA.borrow_mut`
                // and panics ("RefCell already borrowed").
                nav.get().map(|h| h.push(&INSPECTOR, ()));
            });
            children.push(ui! { Button(label = label, on_click = on_click) });
        }
    }

    let nav_rescan = nav;
    let rescan: Rc<dyn Fn()> = Rc::new(move || {
        nav_rescan.get().map(|h| h.reset(&PICKER, ()));
    });
    children.push(ui! { Button(label = "Rescan".to_string(), on_click = rescan) });

    ui! { Stack(gap = StackGap::Md, padding = StackPadding::Lg) { children } }
}

/// The live inspector screen: a status header, a tab strip, and the active
/// tab's panel. Selection / expand / filter state lives in signals here so
/// it survives panel rebuilds and tab switches.
fn inspector_page(snapshot: Signal<Snapshot>, nav: Ref<StackHandle>) -> Element {
    let tab: Signal<usize> = signal!(0);
    let sel_node: Signal<Option<u64>> = signal!(None);
    let expanded: Signal<HashSet<u64>> = signal!(HashSet::new());
    let log_filter: Signal<String> = signal!(String::new());
    let invoke_arg: Signal<String> = signal!(String::new());

    let header = text(move || format::header(&snapshot.get())).into_element();

    let tabs = vec![Tab::new("Elements"), Tab::new("Logs"), Tab::new("Stats")];
    let on_tab: Rc<dyn Fn(usize)> = Rc::new(move |i| tab.set(i));
    let strip = ui! { Tabs(tabs = tabs, active = tab, on_change = on_tab) };

    let back: Rc<dyn Fn()> = Rc::new(move || {
        nav.get().map(|h| h.pop());
    });
    let back_btn = ui! { Button(label = "Disconnect".to_string(), on_click = back) };

    let body = ui! {
        InspectorBody(
            snapshot = snapshot,
            tab = tab,
            sel_node = sel_node,
            expanded = expanded,
            log_filter = log_filter,
            invoke_arg = invoke_arg,
        )
    };

    ui! {
        scroll_view {
            Stack(gap = StackGap::Sm, padding = StackPadding::Sm) {
                header
                strip
                body
                back_btn
            }
        }
    }
}

/// Props for [`InspectorBody`]. All-`Signal` so `#[derive(Default)]` holds
/// (`Signal<T>: Default` for every `T`).
#[derive(Default)]
struct InspectorBodyProps {
    snapshot: Signal<Snapshot>,
    tab: Signal<usize>,
    sel_node: Signal<Option<u64>>,
    expanded: Signal<HashSet<u64>>,
    log_filter: Signal<String>,
    invoke_arg: Signal<String>,
}

/// Reactive tab switch. The scrutinee reads `tab.get()`, so the `ui!`
/// `match` lowers to `runtime_core::switch(...)` and rebuilds the active
/// arm whenever the tab changes. Patterns use guards on the bound `&usize`.
#[component]
fn InspectorBody(props: InspectorBodyProps) -> Element {
    let InspectorBodyProps { snapshot, tab, sel_node, expanded, log_filter, invoke_arg } = props;
    ui! {
        match tab.get() {
            n if *n == 1 => {
                LogsPanel(snapshot = snapshot, filter = log_filter)
            }
            n if *n == 2 => {
                StatsPanel(snapshot = snapshot)
            }
            _ => {
                ElementsPanel(snapshot = snapshot, selected = sel_node, expanded = expanded, invoke_arg = invoke_arg)
            }
        }
    }
}
