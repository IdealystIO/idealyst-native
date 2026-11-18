//! A shell with tabs: ONE `Page` whose variant follows the tab on show.
//! The tabs take no `Page` of their own; changing tab restyles the
//! frame and leaves the header where it is.

use std::rc::Rc;

use idea_ui::{Stack, StackGap, Tab, Tabs, Typography};
use idea_ui_layout::{LabelValue, Page, PageHeader, PageVariant, Section, StatGrid};
use runtime_core::{component, rx, signal, stylesheet, ui, AlignItems, Element, FlexDirection, JustifyContent, Length};

use super::Stat;
use crate::Shell;

/// A row's height in the demo's plain list.
const ROW_HEIGHT: f32 = 44.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ProjectTab {
    #[default]
    Overview,
    Headings,
    Schedule,
}

impl ProjectTab {
    const ALL: [ProjectTab; 3] = [ProjectTab::Overview, ProjectTab::Headings, ProjectTab::Schedule];

    fn label(self) -> &'static str {
        match self {
            ProjectTab::Overview => "Overview",
            ProjectTab::Headings => "Headings",
            ProjectTab::Schedule => "Schedule",
        }
    }

    /// The layout each tab takes, from the same list a screen picks
    /// from.
    fn variant(self) -> PageVariant {
        match self {
            ProjectTab::Overview => PageVariant::Content,
            ProjectTab::Headings => PageVariant::Table,
            ProjectTab::Schedule => PageVariant::Blank,
        }
    }
}

#[component]
pub(crate) fn ShellTabsScreen(#[prop(static)] shell: Shell) -> Element {
    let tab = signal(ProjectTab::Overview);
    let tabs = signal(ProjectTab::ALL.iter().map(|t| Tab::new(t.label(), t.label())).collect::<Vec<_>>());
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| {
        if let Some(picked) = ProjectTab::ALL.iter().find(|t| t.label() == id) {
            tab.set(*picked);
        }
    });
    ui! {
        Page(
            live_variant = Some(rx!(tab.get().variant())),
            header = ui! { PageHeader(title = "North Ramp".to_string(), leading = Some(shell.leading())) },
            toolbar = Some(ui! { Tabs(tabs = tabs, active = rx!(tab.get().label().to_string()), on_change = on_change) }),
            body = ui! {
                // The seat a navigator's outlet takes in a shell: it
                // fills the frame, and a bounded tab is bounded by it.
                view(style = Outlet()) {
                    match tab.get() {
                        ProjectTab::Overview => {
                            Stack(gap = StackGap::Lg) {
                                StatGrid {
                                    Stat(label = "Advance this week".to_string(), value = "42 m".to_string())
                                    Stat(label = "Shifts logged".to_string(), value = "14".to_string())
                                }
                                for part in ["Scope", "Client", "Crew", "Equipment", "Safety"], key = part.to_string() {
                                    Section(title = part.to_string()) {
                                        Stack(gap = StackGap::Sm) {
                                            LabelValue(label = "Owner".to_string(), value = Some("Dana Okafor".to_string()))
                                            LabelValue(label = "Updated".to_string(), value = Some("3 Nov".to_string()))
                                        }
                                    }
                                }
                            }
                        }
                        ProjectTab::Headings => {
                            scroll_view(style = Rows()) {
                                for i in 1..=40, key = i {
                                    view(style = Row()) { Typography(content = format!("Heading {i}")) }
                                }
                            }
                        }
                        ProjectTab::Schedule => {
                            view(style = Grid()) {
                                Typography(content = "A custom grid, edge to edge".to_string(), muted = true)
                            }
                        }
                    }
                }
            },
        )
    }
}

stylesheet! {
    Outlet<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_shrink: 1.0,
            flex_basis: Length::Px(0.0),
            min_width: Length::Px(0.0),
            min_height: Length::Px(0.0),
        }
    }
}

stylesheet! {
    Rows<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_height: Length::Px(0.0),
        }
    }
}

stylesheet! {
    Row<idea_ui::IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            min_height: Length::Px(ROW_HEIGHT),
        }
    }
}

stylesheet! {
    Grid<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            flex_grow: 1.0,
            background: t.color.surface_alt(),
        }
    }
}
