//! A shell with tabs: ONE `Page` whose variant follows the tab on show.
//! The tabs take no `Page` of their own; changing tab restyles the
//! frame and leaves the header where it is.

use std::rc::Rc;

use icons_lucide::LIST_FILTER;
use idea_ui::{Stack, StackGap, Tab, Typography};
use idea_ui_layout::{
    LabelValue, Page, PageHeader, PageVariant, Section, StatGrid, Toolbar, ToolbarButton, ToolbarSearch, ToolbarTabs,
};
use runtime_core::{component, rx, signal, stylesheet, ui, AlignItems, Element, FlexDirection, JustifyContent, Length};

use super::Stat;
use crate::{Demo, Shell};

/// A row's height in the demo's plain list.
const ROW_HEIGHT: f32 = 44.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ProjectTab {
    #[default]
    Overview,
    Headings,
    Schedule,
    Crew,
    Timesheets,
    Reports,
    Contacts,
    Equipment,
}

impl ProjectTab {
    const ALL: [ProjectTab; 8] = [
        ProjectTab::Overview,
        ProjectTab::Headings,
        ProjectTab::Schedule,
        ProjectTab::Crew,
        ProjectTab::Timesheets,
        ProjectTab::Reports,
        ProjectTab::Contacts,
        ProjectTab::Equipment,
    ];

    fn label(self) -> &'static str {
        match self {
            ProjectTab::Overview => "Overview",
            ProjectTab::Headings => "Headings",
            ProjectTab::Schedule => "Schedule",
            ProjectTab::Crew => "Crew",
            ProjectTab::Timesheets => "Timesheets",
            ProjectTab::Reports => "Reports",
            ProjectTab::Contacts => "Contacts",
            ProjectTab::Equipment => "Equipment",
        }
    }

    /// The layout each tab takes, from the same list a screen picks
    /// from.
    fn variant(self) -> PageVariant {
        match self {
            ProjectTab::Schedule => PageVariant::Blank,
            ProjectTab::Overview | ProjectTab::Reports => PageVariant::Content,
            _ => PageVariant::Table,
        }
    }
}

#[component]
pub(crate) fn ShellTabsScreen(#[prop(static)] shell: Shell) -> Element {
    let tab = signal(ProjectTab::Overview);
    let search = signal(String::new());
    let tabs = signal(ProjectTab::ALL.iter().map(|t| Tab::new(t.label(), t.label())).collect::<Vec<_>>());
    let on_change: Rc<dyn Fn(String)> = Rc::new(move |id| {
        if let Some(picked) = ProjectTab::ALL.iter().find(|t| t.label() == id) {
            tab.set(*picked);
        }
    });
    ui! {
        Page(
            live_variant = Some(rx!(tab.get().variant())),
            header = ui! { PageHeader(
                title = "North Ramp".to_string(),
                nav = Some(shell.way_back(vec![shell.crumb("Projects", Demo::Table)], Demo::Table)),
                leading = Some(shell.leading()), status = move || shell.style_picker()) },
            toolbar = Some(ui! {
                Toolbar(
                    start = Some(ui! {
                        ToolbarTabs(tabs = tabs, active = rx!(tab.get().label().to_string()), on_change = on_change)
                    }),
                    end = Some(ui! {
                        ToolbarSearch(value = search, placeholder = "Search this project".to_string())
                        ToolbarButton(label = "Filter".to_string(), icon = Some(LIST_FILTER), count = 2_usize)
                    }),
                )
            }),
            body = ui! {
                // The seat a navigator's outlet takes in a shell: it
                // fills the frame, and a bounded tab is bounded by it.
                view(style = Outlet()) {
                    match tab.get() {
                        ProjectTab::Overview | ProjectTab::Reports => {
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
                        ProjectTab::Schedule => {
                            view(style = Grid()) {
                                Typography(content = "A custom grid, edge to edge".to_string(), muted = true)
                            }
                        }
                        _ => {
                            scroll_view(style = Rows()) {
                                for i in 1..=40, key = i {
                                    view(style = Row()) { Typography(content = format!("Heading {i}")) }
                                }
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
