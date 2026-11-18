//! The two bounded page variants: `Table`, whose list owns the one
//! scroller under a pinned toolbar, and `Blank`, which decides nothing.

use std::rc::Rc;

use icons_lucide::PLUS;
use idea_ui::{Card, Field, Stack, StackGap, Typography};
use idea_ui_layout::{Action, Blank, ListPhase, ListStates, Page, PageHeader, PageVariant};
use runtime_core::{
    component, rx, signal, stylesheet, ui, AlignItems, DisplayKind, Element, FlexDirection, Length, Signal,
    TrackSize,
};

use crate::Shell;

/// A row's height in the demo's plain list.
const ROW_HEIGHT: f32 = 44.0;

fn headings() -> Vec<String> {
    (1..=40).map(|i| format!("Heading {i}")).collect()
}

/// A `Table` page. The header's ⋮ puts the list into each of its
/// states; the search field is the toolbar that stays through all of
/// them.
#[component]
pub(crate) fn TableScreen(#[prop(static)] shell: Shell) -> Element {
    let rows: Signal<Vec<String>> = signal(headings());
    let phase: Signal<ListPhase> = signal(ListPhase::Ready);
    let search = signal(String::new());
    let shown = move || {
        let term = search.get().to_lowercase();
        rows.get().into_iter().filter(|r| r.to_lowercase().contains(&term)).collect::<Vec<_>>()
    };
    let add: Rc<dyn Fn()> = Rc::new(move || {
        let mut all = rows.get();
        all.push(format!("Heading {}", all.len() + 1));
        rows.set(all);
        phase.set(ListPhase::Ready);
    });
    let show_rows: Rc<dyn Fn()> = Rc::new(move || {
        rows.set(headings());
        phase.set(ListPhase::Ready);
    });
    let show_blank: Rc<dyn Fn()> = Rc::new(move || {
        rows.set(Vec::new());
        search.set(String::new());
        phase.set(ListPhase::Ready);
    });
    let show_loading: Rc<dyn Fn()> = Rc::new(move || phase.set(ListPhase::Loading));
    let show_failed: Rc<dyn Fn()> =
        Rc::new(move || phase.set(ListPhase::Failed("The server did not answer.".to_string())));
    ui! {
        Page(
            variant = PageVariant::Table,
            header = ui! {
                PageHeader(
                    title = "Headings".to_string(),
                    leading = Some(shell.leading()),
                    actions = vec![Action::new("Add heading", add.clone()).icon(PLUS).primary()],
                    menu = vec![
                        Action::new("Show rows", show_rows.clone()),
                        Action::new("Show loading", show_loading.clone()),
                        Action::new("Show failed", show_failed.clone()),
                        Action::new("Show blank", show_blank.clone()),
                    ],
                )
            },
            toolbar = Some(ui! { Field(value = search, placeholder = Some("Search headings (try \"zz\" for the empty state)".to_string())) }),
            body = ui! {
                ListStates(
                    phase = rx!(phase.get()),
                    is_empty = rx!(shown().is_empty()),
                    narrowed = rx!(!search.get().is_empty()),
                    blank = Blank::new("No headings yet")
                        .body("A heading is one face the crews advance.")
                        .action(Action::new("Add heading", add.clone()).icon(PLUS)),
                    no_match = "No headings match.".to_string(),
                    failed_title = "Couldn't load the headings".to_string(),
                    on_retry = Some(show_rows.clone()),
                    rows = move || ui! {
                        scroll_view(style = Rows()) {
                            for name in shown(), key = name.clone() {
                                view(style = Row()) { Typography(content = name.clone()) }
                            }
                        }
                    },
                )
            },
        )
    }
}

/// A `Blank` page: a board that runs edge to edge and brings its own
/// scroller and its own insets.
#[component]
pub(crate) fn BlankScreen(#[prop(static)] shell: Shell) -> Element {
    ui! {
        Page(
            variant = PageVariant::Blank,
            header = ui! { PageHeader(title = "Crew board".to_string(), leading = Some(shell.leading())) },
            body = ui! {
                scroll_view(style = Board()) {
                    view(style = BoardGrid()) {
                        for crew in ["A crew", "B crew", "C crew", "Night crew", "Services", "Maintenance"], key = crew.to_string() {
                            Card {
                                Stack(gap = StackGap::Sm) {
                                    Typography(content = crew.to_string(), kind = idea_ui::typography_kind::H3)
                                    Typography(content = "6 workers · North Ramp".to_string(), muted = true)
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
    // The Table page's one scroller: bounded by the page, not by its
    // rows.
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
    // A Blank body owns its whole area: its scroller, its ground and
    // its insets are its own.
    Board<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_height: Length::Px(0.0),
            background: t.color.surface_alt(),
        }
    }
}

stylesheet! {
    // Particular to this screen, so a screen-local overlay is allowed
    // (rule `layout`): the board's own column count.
    BoardGrid<idea_ui::IdeaThemeRef> {
        base(t) {
            display: DisplayKind::Grid,
            grid_template_columns: vec![TrackSize::Fr(1.0)],
            gap: t.spacing.md(),
            padding: t.spacing.lg(),
        }
        breakpoint sm(_t) {
            grid_template_columns: vec![TrackSize::Fr(1.0), TrackSize::Fr(1.0)],
        }
        breakpoint lg(_t) {
            grid_template_columns: vec![TrackSize::Fr(1.0), TrackSize::Fr(1.0), TrackSize::Fr(1.0)],
        }
    }
}
