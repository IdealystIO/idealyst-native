//! Idealyst Inspector — a runtime debugging dashboard for idealyst apps.
//!
//! Shows, live, for any running app:
//!
//! - **Components** — every mounted `#[component]` in its rendered
//!   hierarchy, optionally interleaved with the primitive elements; the
//!   selected one's props (each marked Live / Static / Handler …, with its
//!   current value), its `#[method]`s (invokable), and its root element's
//!   frame and native read-back;
//! - **Signals** — watched signals with write counts and recent history,
//!   settable when registered with `watch_signal_writable`;
//! - **Navigation** — every navigator's back stack and query state, and
//!   push / replace / reset / pop;
//! - **Logs & perf** — the captured log stream and phase timers.
//!
//! ## Front end only
//!
//! This crate is the Inspector's front end. It never talks to an app.
//! The **Inspector server** (`inspector-server`, hosted by `idealyst
//! inspect`) discovers the running apps, holds each app's robot-bridge
//! connection, polls and accumulates its state and runs the actions. The
//! front end renders what the server pushes over one WebSocket and sends
//! back what the user did ([`bridge::client`]). Because it needs nothing
//! but that socket, the same front end runs in a browser. The CLI embeds
//! its web build and serves it from the Inspector server.
//!
//! ## Run it
//!
//! ```text
//! idealyst dev --web --local crates/dev/robot-e2e/examples/conformance --inspect
//! #   builds + runs the target and opens the Inspector on it in the browser
//! idealyst inspect
//! #   just the Inspector: every running app, at http://127.0.0.1:9719
//! ```
//!
//! The desktop build connects to the same server (`ws://127.0.0.1:9719/ws`
//! unless `IDEALYST_INSPECT_URL` says otherwise; `IDEALYST_INSPECT_APP`
//! names an app id to open on, as `?app=` does in the browser).

use std::cell::RefCell;
use std::rc::Rc;

use idea_ui::{dark_theme, install_idea_theme_reactive, light_theme};
use runtime_core::{component, effect, signal, ui, Element, Signal};
use serde_json::Value;

pub mod bridge;
mod ui;

use bridge::client::{self, Focus, ServerLink, Sinks};
use bridge::endpoint;
use bridge::model::{AppInfo, Snapshot};
use ui::connect::Connect;
use ui::shell::Shell;

/// The connected target, as the sidebar names it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Target {
    pub name: String,
    /// `macOS · pid 48213`, or the address for a manual connection.
    pub detail: String,
}

impl Target {
    fn from_app(app: &AppInfo) -> Target {
        let platform = app.platform.as_deref().map(platform_label).unwrap_or("app");
        Target { name: app.name.clone(), detail: format!("{platform} · pid {}", app.pid) }
    }

    fn from_addr(addr: &str) -> Target {
        Target { name: addr.to_string(), detail: "connected by address".to_string() }
    }
}

/// `macos` → `macOS`.
pub fn platform_label(platform: &str) -> &str {
    match platform {
        "macos" => "macOS",
        "ios" => "iOS",
        "android" => "Android",
        "web" => "Web",
        "linux" => "Linux",
        "windows" => "Windows",
        other => other,
    }
}

/// What the per-item verbs fetch (the selected component / signal).
pub(crate) fn set_focus(focus: Focus) {
    client::set_focus(focus);
}

/// Run an action verb on the attached app; its outcome shows as
/// `Snapshot::last_action`.
pub(crate) fn action(label: impl Into<String>, cmd: &str, args: Value) {
    client::action(label, cmd, args);
}

/// SDK-handler registration seam the CLI-generated wrapper calls after
/// `runtime_vocabulary::register_builtins`. Nothing extra to register.
pub fn register_scene_extensions<H: runtime_scene::Host>(_registry: &mut runtime_scene::Registry<H>) {}

#[component]
pub fn app() -> Element {
    // Dark by default (the Inspector was designed dark); the sidebar and
    // picker toggles flip it, and idea-ui re-themes from the signal.
    let dark: Signal<bool> = signal(true);
    install_idea_theme_reactive(move || if dark.get() { dark_theme() } else { light_theme() });

    let snapshot: Signal<Snapshot> = signal(Snapshot::default());
    let apps: Signal<Vec<AppInfo>> = signal(Vec::new());
    let link: Signal<ServerLink> = signal(ServerLink::default());
    let target: Signal<Option<Target>> = signal(None);
    client::start(endpoint::server_url(), Sinks { link, apps, snapshot });

    // Plain closures over `Copy` signals are themselves `Copy`, so the
    // reactive branch below can wrap a fresh `Rc` on every rebuild.
    let on_connect = move |app: Option<AppInfo>, addr: String| {
        snapshot.set(Snapshot::default());
        match app {
            Some(app) => {
                client::attach(app.id.clone());
                target.set(Some(Target::from_app(&app)));
            }
            None => {
                client::attach_addr(addr.clone());
                target.set(Some(Target::from_addr(&addr)));
            }
        }
    };
    let on_disconnect = move || {
        client::detach();
        target.set(None);
    };

    // An app the launcher named opens as soon as the server lists it.
    let pending = Rc::new(RefCell::new(endpoint::initial_app()));
    effect(move || {
        let listed = apps.get();
        let Some(id) = pending.borrow().clone() else { return };
        if let Some(app) = listed.into_iter().find(|a| a.id == id) {
            pending.borrow_mut().take();
            on_connect(Some(app), String::new());
        }
    });

    let connected = runtime_core::memo(move || target.get().is_some());
    let target_now = runtime_core::memo(move || target.get().unwrap_or_default());

    ui! {
        view(style = ui::styles::Root()) {
            if connected {
                Shell(
                    snapshot = snapshot,
                    target = target_now,
                    dark = dark,
                    on_disconnect = Rc::new(on_disconnect) as Rc<dyn Fn()>,
                )
            } else {
                Connect(
                    apps = apps.read_only(),
                    link = link.read_only(),
                    dark = dark,
                    on_connect = Rc::new(on_connect) as Rc<dyn Fn(Option<AppInfo>, String)>,
                )
            }
        }
    }
}
