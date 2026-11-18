//! The overlays: each is anchored or centred with a pointer and a
//! bottom sheet on a phone. Open each one at 390 and at 1440.

use std::rc::Rc;

use icons_lucide::{COPY, PENCIL, TRASH};
use idea_ui::{variant, AnchorTarget, Button, Field, Stack, StackAlign, StackAxis, StackGap, Typography};
use idea_ui_layout::{
    dialog_block, Action, ActionMenu, ActionMenuSurface, AdaptivePopover, Dialog, DialogFooter, DrillCx, DrillSheet,
    FieldGrid, Page, PageHeader, Section, Sheet, SheetActionRow, SheetCheckRow, SheetHeading, SheetPushRow,
};
use runtime_core::{component, rx, signal, ui, Element, PressableHandle, Ref, Signal};

use super::noop;
use crate::Shell;

const GROUPINGS: [&str; 3] = ["Heading", "Shift", "Crew"];

fn row_actions() -> Vec<Action> {
    vec![
        Action::new("Rename", noop()).icon(PENCIL),
        Action::new("Duplicate", noop()).icon(COPY),
        Action::new("Delete", noop()).icon(TRASH),
    ]
}

#[component]
pub(crate) fn OverlaysScreen(#[prop(static)] shell: Shell) -> Element {
    // A menu opened from the screen's own trigger.
    let menu_open = signal(false);
    let menu_button: Ref<PressableHandle> = Ref::new();
    let open_menu: Rc<dyn Fn()> = Rc::new(move || menu_open.set(true));

    // A popover of free-form content.
    let filter_open = signal(false);
    let filter_button: Ref<PressableHandle> = Ref::new();
    let open_filter: Rc<dyn Fn()> = Rc::new(move || filter_open.set(true));
    let day = signal(true);
    let night = signal(false);
    let toggle_day: Rc<dyn Fn()> = Rc::new(move || day.set(!day.get()));
    let toggle_night: Rc<dyn Fn()> = Rc::new(move || night.set(!night.get()));

    // A sheet with levels.
    let drill_open = signal(false);
    let level: Signal<Option<usize>> = signal(None);
    let grouping = signal(0_usize);
    let archived = signal(false);
    let open_drill: Rc<dyn Fn()> = Rc::new(move || {
        level.set(None);
        drill_open.set(true);
    });
    let close_drill: Rc<dyn Fn()> = Rc::new(move || drill_open.set(false));
    let drill_root: Rc<dyn Fn(DrillCx) -> Element> = Rc::new(move |cx| {
        let push = cx.push.clone();
        let to_grouping: Rc<dyn Fn()> = Rc::new(move || push(0));
        let toggle: Rc<dyn Fn()> = Rc::new(move || archived.set(!archived.get()));
        ui! {
            Stack(gap = StackGap::Xs) {
                SheetHeading(text = "View options".to_string())
                SheetPushRow(
                    label = "Group by".to_string(),
                    summary = rx!(GROUPINGS[grouping.get()].to_string()),
                    on_press = to_grouping,
                )
                SheetCheckRow(label = "Include archived".to_string(), checked = rx!(archived.get()), on_press = toggle)
            }
        }
    });
    let drill_title: Rc<dyn Fn(usize) -> String> = Rc::new(|_| "Group by".to_string());
    let drill_pane: Rc<dyn Fn(usize, DrillCx) -> Element> = Rc::new(move |_, cx| {
        let back = cx.back.clone();
        ui! {
            Stack(gap = StackGap::Xs) {
                for choice in GROUPINGS.iter().enumerate(), key = choice.0 {
                    SheetCheckRow(
                        label = choice.1.to_string(),
                        checked = { let i = choice.0; rx!(grouping.get() == i) },
                        on_press = {
                            let i = choice.0;
                            let back = back.clone();
                            let pick: Rc<dyn Fn()> = Rc::new(move || {
                                grouping.set(i);
                                back();
                            });
                            pick
                        },
                    )
                }
            }
        }
    });

    // A form dialog.
    let dialog_open = signal(false);
    let name = signal("Bolts used".to_string());
    let unit = signal("each".to_string());
    let open_dialog: Rc<dyn Fn()> = Rc::new(move || dialog_open.set(true));
    let close_dialog: Rc<dyn Fn()> = Rc::new(move || dialog_open.set(false));
    let cancel = close_dialog.clone();
    let save = close_dialog.clone();

    // A plain sheet, at every width.
    let sheet_open = signal(false);
    let open_sheet: Rc<dyn Fn()> = Rc::new(move || sheet_open.set(true));
    let close_sheet: Rc<dyn Fn()> = Rc::new(move || sheet_open.set(false));
    let sheet_done = close_sheet.clone();

    ui! {
        Page(
            header = ui! { PageHeader(title = "Overlays".to_string(), leading = Some(shell.leading()), status = move || shell.style_picker()) },
            body = ui! {
                Stack(gap = StackGap::Lg) {
                    Section(title = "ActionMenu".to_string(), subtitle = Some("A ⋮ and its verbs: an anchored menu, or an action sheet on a phone".to_string())) {
                        Stack(axis = StackAxis::Row, gap = StackGap::Sm, align = StackAlign::Center) {
                            Typography(content = "Heading 12".to_string())
                            ActionMenu(actions = row_actions(), heading = Some("Heading 12".to_string()))
                        }
                    }
                    Section(title = "ActionMenuSurface".to_string(), subtitle = Some("The same surface behind a trigger of the screen's own".to_string())) {
                        Stack(axis = StackAxis::Row) {
                            Button(label = "Row actions", variant = variant::Outlined, on_click = open_menu.clone(), bind_to = Some(menu_button))
                        }
                    }
                    Section(title = "AdaptivePopover".to_string(), subtitle = Some("Free-form content: a popover, or a titled sheet on a phone".to_string())) {
                        Stack(axis = StackAxis::Row) {
                            Button(label = "Filter shifts", variant = variant::Outlined, on_click = open_filter.clone(), bind_to = Some(filter_button))
                        }
                    }
                    Section(title = "DrillSheet".to_string(), subtitle = Some("A sheet whose rows push panes: the phone form of a menu with submenus".to_string())) {
                        Stack(axis = StackAxis::Row) {
                            Button(label = "View options", variant = variant::Outlined, on_click = open_drill.clone())
                        }
                    }
                    Section(title = "Dialog".to_string(), subtitle = Some("A form: a top-anchored card, or a sheet on a phone".to_string())) {
                        Stack(axis = StackAxis::Row) {
                            Button(label = "Edit field", variant = variant::Outlined, on_click = open_dialog.clone())
                        }
                    }
                    Section(title = "Sheet".to_string(), subtitle = Some("The bottom sheet itself, at every width".to_string())) {
                        Stack(axis = StackAxis::Row) {
                            Button(label = "Open a sheet", variant = variant::Outlined, on_click = open_sheet.clone())
                        }
                    }
                }
            },
        ) {
            ActionMenuSurface(open = menu_open, target = Some(AnchorTarget::from(menu_button)), actions = row_actions())
            AdaptivePopover(
                open = filter_open,
                target = Some(AnchorTarget::from(filter_button)),
                title = Some("Filter shifts".to_string()),
                content = move || ui! {
                    Stack(gap = StackGap::Xs) {
                        SheetCheckRow(label = "Day".to_string(), checked = rx!(day.get()), on_press = toggle_day.clone())
                        SheetCheckRow(label = "Night".to_string(), checked = rx!(night.get()), on_press = toggle_night.clone())
                    }
                },
            )
            DrillSheet(
                open = rx!(drill_open.get()),
                on_dismiss = close_drill.clone(),
                level = level,
                root = drill_root.clone(),
                pane_title = drill_title.clone(),
                pane = drill_pane.clone(),
            )
            Dialog(
                open = dialog_open,
                title = "Edit field".to_string(),
                on_close = close_dialog.clone(),
                body = move || ui! {
                    FieldGrid(columns = 2_usize) {
                        Field(label = Some("Label".to_string()), value = name)
                        Field(label = Some("Unit".to_string()), value = unit)
                    }
                },
                footer = move || ui! {
                    DialogFooter {
                        Button(label = "Cancel", variant = variant::Ghost, block = dialog_block(), on_click = cancel.clone())
                        Button(label = "Save", block = dialog_block(), on_click = save.clone())
                    }
                },
            )
            Sheet(
                open = rx!(sheet_open.get()),
                on_dismiss = close_sheet.clone(),
                content = move || ui! {
                    Stack(gap = StackGap::Xs) {
                        SheetHeading(text = "Heading 12".to_string())
                        SheetActionRow(label = "Open".to_string(), on_press = sheet_done.clone())
                        SheetActionRow(label = "Share".to_string(), on_press = sheet_done.clone())
                    }
                },
            )
        }
    }
}
