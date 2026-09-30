//! The app picker: every running app the Inspector server found on this
//! machine, and connect-by-address.

use std::rc::Rc;

use idea_ui::{tone, typography_kind, variant, Badge, Button, Field, FieldSize, Typography};
use runtime_core::{component, memo, signal, ui, Element, ReadSignal, Signal};

use super::styles::{AppRow, Caption, Column, ComponentName, ConnectColumn, Grow, MonoMuted, RowBetween, RowStart, TableBox};
use crate::bridge::client::{self, ServerLink};
use super::theme_toggle::ThemeToggle;
use crate::bridge::model::AppInfo;

#[component]
pub fn Connect(
    apps: ReadSignal<Vec<AppInfo>>,
    link: ReadSignal<ServerLink>,
    /// The theme mode, owned by the app (see `ThemeToggle`).
    dark: Signal<bool>,
    #[prop(default = Rc::new(|_, _| {}) as Rc<dyn Fn(Option<AppInfo>, String)>)]
    on_connect: Rc<dyn Fn(Option<AppInfo>, String)>,
) -> Element {
    let empty = memo(move || apps.get().is_empty());
    let server_line = memo(move || server_line(&link.get()));
    let rescan = Rc::new(client::rescan) as Rc<dyn Fn()>;
    let addr = signal(String::new());
    let on_addr = Rc::new(move |v: String| addr.set(v)) as Rc<dyn Fn(String)>;
    let by_addr = {
        let on_connect = on_connect.clone();
        Rc::new(move || {
            let a = addr.get().trim().to_string();
            if !a.is_empty() {
                on_connect(None, a);
            }
        }) as Rc<dyn Fn()>
    };

    ui! {
        view(style = ConnectColumn()) {
            view(style = Column()) {
                view(style = RowBetween()) {
                    Typography(content = "Idealyst Inspector".to_string(), kind = typography_kind::H1)
                    ThemeToggle(dark = dark)
                }
                Typography(content = "Pick a running app to inspect. Apps started with idealyst dev show up here on their own.".to_string(), muted = true)
            }
            view(style = Column()) {
                view(style = RowBetween()) {
                    Typography(content = "Running on this machine".to_string(), kind = typography_kind::Overline, muted = true)
                    Button(
                        label = "Rescan".to_string(),
                        on_click = rescan,
                        size = idea_ui::size::Sm,
                        tone = tone::Neutral,
                        variant = variant::Outlined,
                        leading_icon = Some(icons_lucide::REFRESH_CW),
                    )
                }
                text(style = Caption()) { server_line }
                if empty {
                    text(style = Caption()) { "No running apps found. Start one with idealyst dev; it shows up here on its own." }
                }
                AppList(apps = apps, on_connect = on_connect)
            }
            view(style = Column()) {
                Typography(content = "Connect by address".to_string(), kind = typography_kind::Overline, muted = true)
                view(style = RowStart()) {
                    Field(
                        value = addr,
                        on_change = on_addr,
                        placeholder = Some("127.0.0.1:53817".to_string()),
                        size = FieldSize::Md,
                        width = Some(320.0),
                    )
                    Button(label = "Connect".to_string(), on_click = by_addr, tone = tone::Neutral, variant = variant::Outlined)
                }
            }
        }
    }
}

/// The Inspector server's state, as the picker says it.
fn server_line(link: &ServerLink) -> String {
    match link {
        ServerLink::Connecting => "Connecting to the Inspector server…".to_string(),
        ServerLink::Connected => "Connected to the Inspector server.".to_string(),
        ServerLink::Down(why) => format!("{why}. Start it with idealyst inspect; this page reconnects on its own."),
        ServerLink::Incompatible { server } => format!(
            "The Inspector server speaks protocol {server}, this Inspector {}. Use the Inspector the server serves.",
            inspector_protocol::PROTOCOL_VERSION
        ),
    }
}

#[component]
fn AppList(
    apps: ReadSignal<Vec<AppInfo>>,
    #[prop(default = Rc::new(|_, _| {}) as Rc<dyn Fn(Option<AppInfo>, String)>)]
    on_connect: Rc<dyn Fn(Option<AppInfo>, String)>,
) -> Element {
    ui! {
        view(style = TableBox()) {
            for app in apps, key = app.id.clone() {
                AppLine(app = app, on_connect = on_connect.clone())
            }
        }
    }
}

#[component]
fn AppLine(
    #[prop(static)] app: AppInfo,
    #[prop(default = Rc::new(|_, _| {}) as Rc<dyn Fn(Option<AppInfo>, String)>)]
    on_connect: Rc<dyn Fn(Option<AppInfo>, String)>,
) -> Element {
    let platform = app.platform.as_deref().map(crate::platform_label).unwrap_or("app").to_string();
    let detail = match &app.project_root {
        Some(root) => format!("pid {} · {} · {root}", app.pid, app.addr()),
        None => format!("pid {} · {}", app.pid, app.addr()),
    };
    let chosen = app.clone();
    let go = Rc::new(move || on_connect(Some(chosen.clone()), chosen.addr())) as Rc<dyn Fn()>;
    ui! {
        view(style = AppRow()) {
            view(style = Grow()) {
                view(style = RowStart()) {
                    text(style = ComponentName()) { app.name.clone() }
                    Badge(label = platform, tone = tone::Neutral, variant = variant::Soft)
                }
                text(style = MonoMuted()) { detail }
            }
            Button(label = "Connect".to_string(), on_click = go)
        }
    }
}
