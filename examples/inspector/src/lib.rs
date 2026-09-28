//! Idealyst Inspector — a runtime debugging dashboard for idealyst apps.
//!
//! Connects to a running app's **robot bridge** (the TCP newline-JSON
//! transport every `idealyst dev` build exposes) and shows, live:
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
//! The Inspector only displays what the bridge reports; everything it
//! knows arrives through [`bridge`], which has no UI in it.
//!
//! ## Run it
//!
//! ```text
//! idealyst dev --macos --local crates/dev/robot-e2e/examples/conformance   # a target
//! idealyst dev --macos --local examples/inspector                          # this app
//! ```
//!
//! Set `IDEALYST_INSPECT_ADDR=127.0.0.1:<port>` to skip the app picker
//! and connect straight to one bridge — the hook a launcher (the CLI)
//! uses to open the Inspector on the app it just started.
//!
//! macOS desktop is the host: raw TCP is trivial there, while wasm cannot
//! open a TCP socket (a web build needs the bridge to speak WebSocket).

use std::cell::RefCell;
use std::rc::Rc;

use idea_ui::{dark_theme, install_idea_theme};
use runtime_core::{component, signal, ui, Element, Signal};
use serde_json::Value;

pub mod bridge;
mod ui;

use bridge::client::{BridgeClient, Focus};
use bridge::discovery::AppInfo;
use bridge::model::Snapshot;
use ui::connect::Connect;
use ui::shell::Shell;

/// How often the UI copies the client's latest snapshot into the
/// reactive signal. The client refreshes on its own; this is only the
/// render cadence.
const POLL_MS: i32 = 250;

/// The environment variable that skips the picker (see the crate docs).
pub const ADDR_ENV: &str = "IDEALYST_INSPECT_ADDR";

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

thread_local! {
    /// The one connected target. The UI reaches it through the functions
    /// below rather than threading a non-`Default` handle through props.
    static CLIENT: RefCell<Option<BridgeClient>> = const { RefCell::new(None) };
}

pub(crate) fn connect(addr: String) {
    CLIENT.with(|c| *c.borrow_mut() = Some(BridgeClient::connect(addr)));
}

pub(crate) fn disconnect() {
    CLIENT.with(|c| *c.borrow_mut() = None);
}

fn client_snapshot() -> Option<Snapshot> {
    CLIENT.with(|c| c.borrow().as_ref().map(|cl| cl.snapshot()))
}

/// What the per-item verbs fetch (the selected component / signal).
pub(crate) fn set_focus(focus: Focus) {
    CLIENT.with(|c| {
        if let Some(cl) = c.borrow().as_ref() {
            cl.set_focus(focus);
        }
    });
}

/// Send an action verb; its outcome shows as `Snapshot::last_action`.
pub(crate) fn action(label: impl Into<String>, cmd: &str, args: Value) {
    CLIENT.with(|c| {
        if let Some(cl) = c.borrow().as_ref() {
            cl.action(label, cmd, args);
        }
    });
}

/// SDK-handler registration seam the CLI-generated wrapper calls after
/// `runtime_vocabulary::register_builtins`. Nothing extra to register.
pub fn register_scene_extensions<H: runtime_scene::Host>(_registry: &mut runtime_scene::Registry<H>) {}

#[component]
pub fn app() -> Element {
    install_idea_theme(dark_theme());

    let snapshot: Signal<Snapshot> = signal(Snapshot::default());
    let target: Signal<Option<Target>> = signal(None);

    if let Ok(addr) = std::env::var(ADDR_ENV) {
        connect(addr.clone());
        target.set(Some(Target::from_addr(&addr)));
    }
    schedule_poll(snapshot);

    // Plain closures over `Copy` signals are themselves `Copy`, so the
    // reactive branch below can wrap a fresh `Rc` on every rebuild.
    let on_connect = move |app: Option<AppInfo>, addr: String| {
        connect(addr.clone());
        snapshot.set(Snapshot::default());
        target.set(Some(match &app {
            Some(app) => Target::from_app(app),
            None => Target::from_addr(&addr),
        }));
    };
    let on_disconnect = move || {
        disconnect();
        target.set(None);
    };
    let connected = runtime_core::memo(move || target.get().is_some());
    let target_now = runtime_core::memo(move || target.get().unwrap_or_default());

    ui! {
        view(style = ui::styles::Root()) {
            if connected {
                Shell(
                    snapshot = snapshot,
                    target = target_now,
                    on_disconnect = Rc::new(on_disconnect) as Rc<dyn Fn()>,
                )
            } else {
                Connect(on_connect = Rc::new(on_connect) as Rc<dyn Fn(Option<AppInfo>, String)>)
            }
        }
    }
}

/// Copy the client's latest snapshot into `snapshot`. The write is
/// equality-guarded, so an unchanged target wakes nothing.
fn schedule_poll(snapshot: Signal<Snapshot>) {
    runtime_core::after_ms_detached(POLL_MS, move || {
        if let Some(snap) = client_snapshot() {
            snapshot.set(snap);
        }
        schedule_poll(snapshot);
    });
}
