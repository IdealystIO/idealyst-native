//! The connected frame: sidebar (target identity, section nav) beside the
//! active section.

use std::rc::Rc;

use idea_ui::{tone, variant, Button, Icon};
use runtime_core::IconData;
use runtime_core::{component, derived, memo, pressable, signal, ui, Element, IntoElement, Signal};

use super::styles::{
    IdentityCard, Main, Mono, MonoMuted, NavItem, NavItemActive, NavItemText, NavItemTextActive,
    ShellRow, Sidebar, Spacer, StatusDot, StatusDotLive,
};
use super::components::ComponentsScreen;
use super::logs::LogsScreen;
use super::navigation::NavigationScreen;
use super::signals::SignalsScreen;
use crate::bridge::model::{Snapshot, Status};
use crate::Target;

/// The four sections.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    Components,
    Signals,
    Navigation,
    Logs,
}

impl Section {
    const ALL: [Section; 4] = [Section::Components, Section::Signals, Section::Navigation, Section::Logs];

    fn label(self) -> &'static str {
        match self {
            Section::Components => "Components",
            Section::Signals => "Signals",
            Section::Navigation => "Navigation",
            Section::Logs => "Logs & perf",
        }
    }

    fn icon(self) -> IconData {
        match self {
            Section::Components => icons_lucide::COMPONENT,
            Section::Signals => icons_lucide::ACTIVITY,
            Section::Navigation => icons_lucide::LAYERS,
            Section::Logs => icons_lucide::SCROLL_TEXT,
        }
    }
}

fn status_line(status: &Status) -> String {
    match status {
        Status::Connecting => "Connecting…".to_string(),
        Status::Live { rtt_ms } => format!("Live · {rtt_ms} ms round trip"),
        Status::Down(err) => format!("Not connected: {err}"),
    }
}

#[component]
pub fn Shell(
    snapshot: Signal<Snapshot>,
    target: Target,
    #[prop(default = Rc::new(|| {}) as Rc<dyn Fn()>)] on_disconnect: Rc<dyn Fn()>,
) -> Element {
    let section: Signal<Section> = signal(Section::Components);
    ui! {
        view(style = ShellRow()) {
            SideNav(snapshot = snapshot, target = target, section = section, on_disconnect = on_disconnect)
            view(style = Main()) {
                match section.get() {
                    Section::Components => {
                        ComponentsScreen(snapshot = snapshot)
                    }
                    Section::Signals => {
                        SignalsScreen(snapshot = snapshot)
                    }
                    Section::Navigation => {
                        NavigationScreen(snapshot = snapshot)
                    }
                    Section::Logs => {
                        LogsScreen(snapshot = snapshot)
                    }
                }
            }
        }
    }
}

#[component]
fn SideNav(
    snapshot: Signal<Snapshot>,
    target: Target,
    section: Signal<Section>,
    #[prop(default = Rc::new(|| {}) as Rc<dyn Fn()>)] on_disconnect: Rc<dyn Fn()>,
) -> Element {
    let name = target.clone();
    let name = memo(move || name.get().name);
    let detail = memo(move || target.get().detail);
    let status = memo(move || status_line(&snapshot.get().status));
    let live = memo(move || matches!(snapshot.get().status, Status::Live { .. }));
    let dot = StatusDot().live(derived(move || if live.get() { StatusDotLive::On } else { StatusDotLive::Off }));
    ui! {
        view(style = Sidebar()) {
            view(style = IdentityCard()) {
                view(style = super::styles::RowStart()) {
                    view(style = dot) {}
                    text(style = super::styles::ComponentName()) { "{name}" }
                }
                text(style = Mono()) { "{detail}" }
                text(style = MonoMuted()) { "{status}" }
            }
            for s in Section::ALL {
                SectionLink(section = section, target = s)
            }
            view(style = Spacer()) {}
            Button(
                label = "Disconnect".to_string(),
                on_click = on_disconnect,
                tone = tone::Neutral,
                variant = variant::Ghost,
                leading_icon = Some(icons_lucide::UNPLUG),
            )
        }
    }
}

/// One sidebar entry.
#[component]
fn SectionLink(section: Signal<Section>, #[prop(static)] target: Section) -> Element {
    let active = memo(move || section.get() == target);
    let row = NavItem().active(derived(move || if active.get() { NavItemActive::On } else { NavItemActive::Off }));
    let label = NavItemText()
        .active(derived(move || if active.get() { NavItemTextActive::On } else { NavItemTextActive::Off }));
    let body = ui! {
        view(style = row) {
            Icon(data = target.icon(), size = 18.0)
            text(style = label) { target.label() }
        }
    };
    pressable(vec![body], move || section.set(target)).into_element()
}
