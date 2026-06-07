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

use idea_ui::{install_idea_theme, light_theme, Button, Stack, StackGap, StackPadding};
use runtime_core::{
    component, signal, text, text_input, ui, Element, IntoElement, Ref, Route, Screen, Signal,
};
use serde_json::{json, Value};
use stack_navigator::{Navigator, StackBuilder, StackHandle, StackScreenExt};

mod client;
mod discovery;
mod format;

use client::{BridgeClient, Snapshot};

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
            snapshot.set(client_snapshot());
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

/// The live inspector. Every panel is a reactive `text` reading `snapshot`,
/// so they re-render on each poll without any list reconciliation. The
/// command bar drives the target back.
fn inspector_page(snapshot: Signal<Snapshot>, nav: Ref<StackHandle>) -> Element {
    // Command-bar inputs (local to this screen).
    let click_id: Signal<String> = signal!(String::new());
    let inv_instance: Signal<String> = signal!(String::new());
    let inv_method: Signal<String> = signal!(String::new());

    // — Header / status —
    let header = text(move || format::header(&snapshot.get())).into_element();

    // — Panels (each a reactive multi-line text) —
    let navigators = panel("NAVIGATORS", move || format::navigators(&snapshot.get()));
    let tree = panel("TREE", move || format::tree(&snapshot.get()));
    let raw = panel("RAW ELEMENTS (find_all)", move || format::raw_elements(&snapshot.get()));
    let components = panel("COMPONENTS", move || format::components(&snapshot.get()));
    let perf = panel("PERF", move || format::perf(&snapshot.get()));
    let signals = panel("SIGNALS", move || format::signals(&snapshot.get()));
    let logs = panel("LOGS", move || format::logs(&snapshot.get()));

    // — Command bar —
    let click_input = text_input(click_id, move |v| click_id.set(v))
        .placeholder("test_id".to_string())
        .into_element();
    let do_click: Rc<dyn Fn()> = Rc::new(move || {
        let id = click_id.get();
        if !id.is_empty() {
            client_action("click_test_id", json!({ "test_id": id }));
        }
    });
    let click_btn = ui! { Button(label = "Click by test_id".to_string(), on_click = do_click) };

    let inst_input = text_input(inv_instance, move |v| inv_instance.set(v))
        .placeholder("instance_id".to_string())
        .into_element();
    let method_input = text_input(inv_method, move |v| inv_method.set(v))
        .placeholder("method".to_string())
        .into_element();
    let do_invoke: Rc<dyn Fn()> = Rc::new(move || {
        if let Ok(instance) = inv_instance.get().trim().parse::<u64>() {
            let method = inv_method.get();
            if !method.is_empty() {
                client_action(
                    "invoke_method",
                    json!({ "instance_id": instance, "method": method, "args": {} }),
                );
            }
        }
    });
    let invoke_btn = ui! { Button(label = "Invoke method".to_string(), on_click = do_invoke) };

    let clear_logs: Rc<dyn Fn()> = Rc::new(move || client_action("clear_logs", json!({})));
    let clear_btn = ui! { Button(label = "Clear logs".to_string(), on_click = clear_logs) };

    let nav_back = nav;
    let back: Rc<dyn Fn()> = Rc::new(move || {
        nav_back.get().map(|h| h.pop());
    });
    let back_btn = ui! { Button(label = "Disconnect".to_string(), on_click = back) };

    let command_bar = ui! {
        Stack(gap = StackGap::Sm, padding = StackPadding::Md) {
            text("Actions").into_element()
            click_input
            click_btn
            inst_input
            method_input
            invoke_btn
            clear_btn
            back_btn
        }
    };

    let body = ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Md) {
            header
            navigators
            tree
            raw
            components
            perf
            signals
            logs
            command_bar
        }
    };

    ui! { scroll_view { body } }
}

/// A titled panel: a bold-ish title line over a reactive body text.
fn panel(title: &'static str, body: impl Fn() -> String + 'static) -> Element {
    let title_el = text(title).into_element();
    let body_el = text(body).into_element();
    ui! {
        Stack(gap = StackGap::Xs, padding = StackPadding::Sm) {
            title_el
            body_el
        }
    }
}
