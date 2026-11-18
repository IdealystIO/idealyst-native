//! The three split modes: `Nav` and `Detail` inside a scrolling page,
//! `Inspector` inside a bounded one (a workbench).

use std::rc::Rc;

use idea_ui::{variant, Button, Field, ListItem, Stack, StackGap, Typography};
use idea_ui_layout::{
    Action, FieldGrid, LabelValue, Page, PageFrame, PageHeader, PageVariant, Section, SplitMode, SplitView,
};
use runtime_core::{component, rx, signal, stylesheet, ui, AlignItems, Element, FlexDirection, JustifyContent, Signal};

use crate::{Demo, Shell};

const SECTIONS: [&str; 5] = ["General", "Divisions", "Positions", "Pay schedules", "Work hour types"];

/// `Nav`: settings. The list and the open section sit beside each other
/// from `Md`; below it the list is the screen, and opening a section
/// replaces it.
#[component]
pub(crate) fn SplitNavScreen(#[prop(static)] shell: Shell) -> Element {
    let open: Signal<Option<&'static str>> = signal(None);
    let frame = PageFrame::new();
    let back: Rc<dyn Fn()> = Rc::new(move || open.set(None));
    let sections = ui! {
        Stack(gap = StackGap::Xs) {
            for name in SECTIONS.iter(), key = name.to_string() {
                ListItem(
                    label = name.to_string(),
                    active = { let name = *name; rx!(open.get() == Some(name)) },
                    on_press = {
                        let name = *name;
                        let go: Rc<dyn Fn()> = Rc::new(move || {
                            open.set(Some(name));
                            frame.scroll_to_top();
                        });
                        Some(go)
                    },
                )
            }
        }
    };
    let destination = ui! {
        Stack(gap = StackGap::Lg) {
            Typography(content = rx!(open.get().unwrap_or(SECTIONS[0]).to_string()), kind = idea_ui::typography_kind::H2)
            Section(title = "Details".to_string()) {
                FieldGrid(columns = 2_usize) {
                    Field(label = Some("Name".to_string()), value = signal("North Ramp".to_string()))
                    Field(label = Some("Code".to_string()), value = signal("NR-4".to_string()))
                }
            }
            Section(title = "Defaults".to_string()) {
                Stack(gap = StackGap::Sm) {
                    LabelValue(label = "Time zone".to_string(), value = Some("America/Toronto".to_string()))
                    LabelValue(label = "Currency".to_string(), value = Some("CAD".to_string()))
                }
            }
        }
    };
    ui! {
        Page(
            frame = Some(frame),
            header = ui! {
                PageHeader(
                    title = "Team settings".to_string(),
                    leading = Some(shell.leading()), status = move || shell.style_picker(),
                    actions = vec![Action::new("All sections", back.clone())],
                )
            },
            body = ui! {
                SplitView(
                    mode = SplitMode::Nav,
                    first = sections,
                    second = destination,
                    open = rx!(open.get().is_some()),
                    frame = Some(frame),
                    grow = true,
                )
            },
        )
    }
}

/// `Detail`: a record and its companion panel. Beside from `Md`, the
/// panel pinned while the record scrolls; below it the panel stacks
/// under the record.
#[component]
pub(crate) fn SplitDetailScreen(#[prop(static)] shell: Shell) -> Element {
    let frame = PageFrame::new();
    ui! {
        Page(
            frame = Some(frame),
            header = ui! { PageHeader(
                title = "Incident 24-118".to_string(),
                nav = Some(shell.way_back(vec![
                    shell.crumb("Projects", Demo::Table),
                    shell.crumb("North Ramp", Demo::ShellTabs),
                    shell.crumb("Incidents", Demo::Table),
                ], Demo::ShellTabs)),
                leading = Some(shell.leading()), status = move || shell.style_picker()) },
            body = ui! {
                SplitView(
                    mode = SplitMode::Detail,
                    frame = Some(frame),
                    first = ui! {
                        Stack(gap = StackGap::Lg) {
                            for part in ["What happened", "Where", "Who was involved", "Immediate actions", "Root cause", "Follow-up"], key = part.to_string() {
                                Section(title = part.to_string()) {
                                    Typography(content = "The record's own sections scroll with the page. The panel beside them holds still, and stacks underneath on a phone.".to_string())
                                }
                            }
                        }
                    },
                    second = ui! {
                        Section(title = "Activity".to_string()) {
                            Stack(gap = StackGap::Sm) {
                                LabelValue(label = "Filed".to_string(), value = Some("3 Nov, 14:20".to_string()))
                                LabelValue(label = "By".to_string(), value = Some("Dana Okafor".to_string()))
                                LabelValue(label = "Reviewed".to_string(), value = None::<String>)
                            }
                        }
                    },
                )
            },
        )
    }
}

/// A workbench: a `Blank` page holding an `Inspector` split. The pane
/// is beside the canvas from `Lg`, overlays it on a tablet while
/// something is selected, and docks under it on a phone.
#[component]
pub(crate) fn WorkbenchScreen(#[prop(static)] shell: Shell) -> Element {
    let selected = signal(false);
    let select: Rc<dyn Fn()> = Rc::new(move || selected.set(true));
    let clear: Rc<dyn Fn()> = Rc::new(move || selected.set(false));
    ui! {
        Page(
            variant = PageVariant::Blank,
            header = ui! { PageHeader(title = "Form builder".to_string(), leading = Some(shell.leading()), status = move || shell.style_picker()) },
            // The canvas is a well (the ground), so the pane is a card in it.
            body = ui! {
              view(style = Well()) {
                SplitView(
                    mode = SplitMode::Inspector,
                    card = true,
                    first = ui! {
                        view(style = Canvas()) {
                            Button(label = "Select the field", variant = variant::Outlined, on_click = select.clone())
                        }
                    },
                    second = ui! {
                        Section(
                            title = "Field settings".to_string(),
                            hint = Some("What a worker sees when they fill this field in.".to_string()),
                            action = Some(ui! { Button(label = "Reset", variant = variant::Ghost) }),
                        ) {
                            Field(label = Some("Label".to_string()), value = signal("Bolts used".to_string()))
                            Field(label = Some("Unit".to_string()), value = signal("each".to_string()))
                        }
                    },
                    open = rx!(selected.get()),
                    on_close = Some(clear.clone()),
                )
              }
            },
        )
    }
}

stylesheet! {
    Canvas<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            flex_grow: 1.0,
        }
    }
}

stylesheet! {
    // The editor's ground: the canvas lies on it and the pane's card
    // floats in it.
    Well<idea_ui::IdeaThemeRef> {
        base(t) {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            min_height: runtime_core::Length::Px(0.0),
            padding: t.spacing.sm(),
            background: t.color.background(),
        }
    }
}
