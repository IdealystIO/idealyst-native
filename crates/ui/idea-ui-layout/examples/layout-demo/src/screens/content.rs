//! A `Content` page: a record's detail. The page scrolls as one column
//! of sections under a header that holds still.

use std::rc::Rc;

use icons_lucide::{ARCHIVE, DOWNLOAD, PENCIL, TRASH};
use idea_ui::{variant, Button, Field, Stack, StackGap, Tag};
use idea_ui_layout::{
    dialog_block, Action, Dialog, DialogFooter, FieldGrid, FieldGridRow, LabelValue, Page, PageHeader, Section,
    StatGrid,
};
use runtime_core::{component, signal, ui, Element};

use super::{noop, Stat};
use crate::{Demo, Shell};

#[component]
pub(crate) fn ContentScreen(#[prop(static)] shell: Shell) -> Element {
    let editing = signal(false);
    let first = signal("Dana".to_string());
    let last = signal("Okafor".to_string());
    let phone = signal("555 0142".to_string());
    let notes = signal(String::new());
    let open_edit: Rc<dyn Fn()> = Rc::new(move || editing.set(true));
    let close_edit: Rc<dyn Fn()> = Rc::new(move || editing.set(false));
    let cancel = close_edit.clone();
    let save = close_edit.clone();
    ui! {
        Page(
            header = ui! {
                PageHeader(
                    title = "Dana Okafor".to_string(),
                    nav = Some(shell.way_back(vec![shell.crumb("Workers", Demo::Table)], Demo::Table)),
                    leading = Some(shell.leading()), status = move || shell.style_picker(),
                    actions = vec![
                        Action::new("Edit worker", open_edit.clone()).icon(PENCIL).primary(),
                        Action::new("Export", noop()).icon(DOWNLOAD),
                    ],
                    menu = vec![
                        Action::new("Archive", noop()).icon(ARCHIVE),
                        Action::new("Delete", noop()).icon(TRASH),
                    ],
                )
            },
            body = ui! {
                Stack(gap = StackGap::Lg) {
                    StatGrid {
                        Stat(label = "Hours this period".to_string(), value = "76.5".to_string())
                        Stat(label = "Shifts".to_string(), value = "9".to_string())
                        Stat(label = "Certifications".to_string(), value = "4".to_string())
                        Stat(label = "Open forms".to_string(), value = "1".to_string())
                    }
                    Section(title = "Employment".to_string(), subtitle = Some("What payroll reads".to_string())) {
                        Stack(gap = StackGap::Sm) {
                            LabelValue(label = "Hired".to_string(), value = Some("12 Mar 2024".to_string()))
                            LabelValue(label = "Position".to_string(), value = Some("Jumbo operator".to_string()))
                            LabelValue(
                                label = "Division".to_string(),
                                value = None::<String>,
                                hint = Some("The crew this worker's hours are costed to.".to_string()),
                            )
                            LabelValue(label = "Status".to_string(), content = Some(ui! { Tag(label = "Active") }))
                        }
                    }
                    Section(title = "Contact".to_string()) {
                        FieldGrid(columns = 2_usize) {
                            Field(label = Some("First name".to_string()), value = first)
                            Field(label = Some("Last name".to_string()), value = last)
                            Field(label = Some("Phone".to_string()), value = phone)
                            FieldGridRow { Field(label = Some("Notes".to_string()), value = notes) }
                        }
                    }
                }
            },
        ) {
            Dialog(
                open = editing,
                title = "Edit worker".to_string(),
                on_close = close_edit.clone(),
                body = move || ui! {
                    FieldGrid(columns = 2_usize) {
                        Field(label = Some("First name".to_string()), value = first)
                        Field(label = Some("Last name".to_string()), value = last)
                        FieldGridRow { Field(label = Some("Notes".to_string()), value = notes) }
                    }
                },
                footer = move || ui! {
                    DialogFooter {
                        Button(label = "Cancel", variant = variant::Ghost, block = dialog_block(), on_click = cancel.clone())
                        Button(label = "Save", block = dialog_block(), on_click = save.clone())
                    }
                },
            )
        }
    }
}
