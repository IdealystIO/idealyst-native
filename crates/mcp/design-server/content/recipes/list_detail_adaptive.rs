//! # List → detail: push/pop on phones, split view on wide screens
//!
//! summary: One stack navigator and one URL scheme, two presentations — a phone pushes the detail over the list and pops back; a wide screen keeps the list beside the detail and replaces it.
//! tags: navigation, stack, list-detail, adaptive, responsive
//! render: / @phone => Projects, Atlas, Beacon, New project, !Select a project
//! render: / @desktop => Projects, Atlas, Beacon, Select a project
//! render: /projects/atlas @phone => Atlas, Launch checklist, !Beacon
//! render: /projects/atlas @desktop => Atlas, Launch checklist, Beacon, New project
//! render: /projects/nope @phone => Project not found
//!
//! The canonical *adaptive* pattern: the same routes mean the same thing at
//! every width, but the presentation changes kind, not just size.
//!
//! | Width | `/` | `/projects/:id` | Selecting a row | Back |
//! | --- | --- | --- | --- | --- |
//! | phone, small tablet | the list | the detail, full screen | `push` | stack bar ‹ / native back gesture pops |
//! | ≥ `WIDE` | list pane + "select a project" | list pane + detail pane | `push` from `/`, `replace` from a detail | none: the list is always there |
//!
//! Why this shape:
//! - **One navigator, one URL scheme.** A deep link to `/projects/atlas`
//!   lands correctly at any width, and resizing (or rotating a tablet)
//!   across `WIDE` never remounts the open detail — the outlet stays in one
//!   place and only the list pane appears or disappears around it.
//! - **`replace` in split view.** Clicking through ten projects side by
//!   side must not leave ten entries to pop through.
//! - **One visible title.** A pushed detail's title is in the stack bar
//!   (`StackHeader`, drawn on every platform), so its page renders
//!   `Page(header = false)`; the root screen hides the bar because its Page
//!   header is the title. The bar's slot carries `safe_area = TOP` — the
//!   header applies no inset of its own, and on a phone it sits under the
//!   notch.
//! - **Unknown ids render a not-found page**, never a panic: every detail
//!   URL is deep-linkable, so every param must be treated as user input.
//!
//! Depends on the `page_layout` recipe (`crate::page_layout`).

use crate::page_layout::{Page, PageWidth, Section};
use idea_ui::{
    tone, typography_kind, variant, Badge, Button, Card, CardPadding, Center, IdeaThemeRef, List,
    ListItem, Stack, StackGap, ToneRef, Typography,
};
use idea_ui_nav::StackHeader;
use runtime_core::{
    current_breakpoint, rx, stylesheet, ui, Breakpoint, Element, FlexDirection, Length, Ref,
    Route, RouteParams, SafeAreaSides, Screen, Signal,
};
use stack_navigator::{header_state, StackBuilder, StackHandle, StackNavigator, StackScreenExt};
use std::collections::HashMap;
use std::rc::Rc;

/// The width at which list and detail sit side by side. Below it there is
/// not room for a readable list (~320) AND a readable detail (~560).
const WIDE: Breakpoint = Breakpoint::Lg;

fn is_wide() -> bool {
    current_breakpoint().get().is_at_least(WIDE)
}

#[derive(Clone)]
struct ProjectId {
    id: String,
}

impl RouteParams for ProjectId {
    fn to_path(&self, pattern: &str) -> String {
        pattern.replace(":id", &self.id)
    }
    fn from_segments(segs: &HashMap<String, String>) -> Option<Self> {
        segs.get("id").map(|id| ProjectId { id: id.clone() })
    }
}

const LIST: Route<()> = Route::<()>::new("projects", "/");
const DETAIL: Route<ProjectId> = Route::<ProjectId>::new("project", "/projects/:id");

/// Demo data. A real app reads this from a `resource` (see the
/// `collection_states` recipe for loading/empty/error).
struct Project {
    id: &'static str,
    name: &'static str,
    status: &'static str,
    summary: &'static str,
}

const PROJECTS: &[Project] = &[
    Project { id: "atlas", name: "Atlas", status: "Active", summary: "Launch checklist and rollout plan." },
    Project { id: "beacon", name: "Beacon", status: "At risk", summary: "Alerting overhaul." },
    Project { id: "cairn", name: "Cairn", status: "Done", summary: "Archive migration." },
];

fn find(id: &str) -> Option<&'static Project> {
    PROJECTS.iter().find(|p| p.id == id)
}

/// Status is a *meaning*, so it maps to a semantic tone — never a palette hue.
fn status_tone(status: &str) -> ToneRef {
    match status {
        "Active" => tone::Success.into(),
        "At risk" => tone::Warning.into(),
        _ => tone::Neutral.into(),
    }
}

stylesheet! {
    /// Fills the navigator: the panes side by side.
    Panes<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Row,
            width: Length::pct(100.0),
            height: Length::pct(100.0),
            min_height: Length::Px(0.0),
            background: t.color.background(),
        }
    }
}

stylesheet! {
    /// The list column in split view: fixed width, its own border.
    ListPane<IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            width: Length::Px(360.0),
            flex_shrink: 0.0,
            height: Length::pct(100.0),
            border_right_width: 1.0,
            border_right_color: t.color.border(),
        }
    }
}

stylesheet! {
    /// The detail column: takes the rest, stacks the bar over the outlet.
    DetailPane<IdeaThemeRef> {
        base(_t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            flex_basis: Length::Px(0.0),
            min_width: Length::Px(0.0),
            min_height: Length::Px(0.0),
        }
    }
}

pub fn app() -> Element {
    idea_ui::install_idea_theme(idea_ui::light_theme());
    let nav: Ref<StackHandle> = Ref::new();

    // The root screen's list only exists below WIDE, where opening a
    // project always pushes.
    let push: Rc<dyn Fn(&'static str)> = Rc::new(move |id| {
        nav.get().map(|h| h.push(&DETAIL, ProjectId { id: id.to_string() })).unwrap_or_default();
    });

    let open_root = push.clone();
    let builder = StackNavigator::new(&LIST)
        .screen(LIST, move |_| {
            let open = open_root.clone();
            // At WIDE the list lives in the layout's pane, so the root
            // screen is the detail pane's empty state.
            let body = ui! {
                if is_wide() {
                    Center {
                        Typography(content = "Select a project", muted = true)
                    }
                } else {
                    ProjectList(on_open = open.clone(), selected = None)
                }
            };
            Screen::new(body).title("Projects").hide_header(true)
        })
        .screen(DETAIL, |p: ProjectId| match find(&p.id) {
            Some(p) => Screen::new(ui! { ProjectDetail(name = p.name, status = p.status, summary = p.summary) }).title(p.name),
            None => Screen::new(not_found()).title("Not found"),
        })
        .layout(move |ctx| {
            let chrome = ctx.screen_chrome;
            let can_go_back = ctx.can_go_back;
            let active_path = ctx.active_path;
            let pop = ctx.pop.clone();
            let depth = ctx.depth;
            // The split-view pane: push from the empty root, REPLACE from an
            // open detail — clicking through projects side by side must not
            // stack up history to pop through. `depth` is read at press time.
            let open: Rc<dyn Fn(&'static str)> = Rc::new(move |id| {
                let params = ProjectId { id: id.to_string() };
                if let Some(h) = nav.get() {
                    if depth.get() > 1 {
                        h.replace(&DETAIL, params);
                    } else {
                        h.push(&DETAIL, params);
                    }
                }
            });
            let outlet = ctx.outlet;
            ui! {
                view(style = Panes()) {
                    if is_wide() {
                        view(style = ListPane()) {
                            ProjectList(on_open = open.clone(), selected = Some(active_path))
                        }
                    }
                    view(style = DetailPane()) {
                        view(safe_area = SafeAreaSides::TOP) {
                            // No back button in split view: the list IS the way back.
                            StackHeader(
                                state = rx!(header_state(&chrome)),
                                show_back = rx!(can_go_back.get() && !is_wide()),
                                on_back = Some(pop.clone()),
                            )
                        }
                        outlet
                    }
                }
            }
        });

    ui! { builder.bind(nav) }
}

/// The list screen's content. `selected` highlights the open project in
/// split view; on a phone the list is never beside its detail, so it
/// highlights nothing.
#[runtime_core::component]
fn ProjectList(
    #[prop(default = Rc::new(|_: &'static str| {}) as Rc<dyn Fn(&'static str)>)] on_open: Rc<dyn Fn(&'static str)>,
    #[prop(static, default = None)] selected: Option<Signal<String>>,
) -> Element {
    let new_project: Rc<dyn Fn()> = Rc::new(|| {});
    ui! {
        Page(
            title = "Projects",
            subtitle = Some("3 projects".to_string()),
            width = PageWidth::Narrow,
            actions = Some(ui! { Button(label = "New project", on_click = new_project) }),
        ) {
            List {
                for project in PROJECTS.iter() {
                    ListItem(
                        label = project.name.to_string(),
                        trailing = Some(ui! {
                            Badge(label = project.status, tone = status_tone(project.status), variant = variant::Soft)
                        }),
                        active = rx!(selected.is_some_and(|s| s.get() == format!("/projects/{}", project.id))),
                        on_press = Some({
                            let on_open = on_open.clone();
                            let id = project.id;
                            Rc::new(move || on_open(id)) as Rc<dyn Fn()>
                        }),
                    )
                }
            }
        }
    }
}

#[runtime_core::component]
fn ProjectDetail(
    name: String,
    #[prop(static, default = "")] status: &'static str,
    summary: String,
) -> Element {
    ui! {
        Page(title = name, header = false) {
            Section(title = "Overview") {
                Card(padding = CardPadding::Md) {
                    Stack(gap = StackGap::Sm) {
                        Badge(label = status, tone = status_tone(status), variant = variant::Soft)
                        Typography(content = summary)
                    }
                }
            }
        }
    }
}

/// File-local, no props, one call site — the one shape that stays a plain fn.
fn not_found() -> Element {
    ui! {
        Center {
            Stack(gap = StackGap::Sm) {
                Typography(content = "Project not found", kind = typography_kind::H2)
                Typography(content = "It may have been deleted, or the link is wrong.", muted = true)
            }
        }
    }
}
