//! `TaskRow` — one task, with a toggle control.

use runtime_core::{component, ui, Element, FontWeight, StyleRules, StyleSheet};
use std::rc::Rc;

use crate::components::card::Card;
use crate::store::{Task, TaskStore};
use crate::style_helpers::{col, px, row, static_sheet};

#[derive(Default)]
pub struct TaskRowProps {
    /// The shared store, so the row's toggle can mutate the source.
    pub store: TaskStore,
    /// The task this row renders.
    pub task: Task,
}

/// A single task inside a [`Card`]: its title (struck through when done),
/// a priority tag, and a button that toggles the task's done state
/// through the [`TaskStore`].
#[component]
pub fn TaskRow(props: &TaskRowProps) -> Element {
    let store = props.store;
    let task = props.task.clone();
    let id = task.id;
    let title = task.title.clone();
    let priority = task.priority.label().to_string();
    let done = task.done;
    let toggle_label = if done { "Mark undone" } else { "Mark done" };
    ui! {
        Card() {
            view(style = row(12.0)) {
                view(style = left_group()) {
                    text(style = title_sheet(done)) { title }
                    text(style = tag_sheet()) { priority }
                }
                button(label = toggle_label, on_click = move || store.toggle(id))
            }
        }
    }
}

fn left_group() -> Rc<StyleSheet> {
    use runtime_core::{AlignItems, FlexDirection};
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: Some(px(10.0)),
        ..Default::default()
    })
}

fn title_sheet(done: bool) -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(15.0)),
        font_weight: Some(FontWeight::Medium),
        color: Some(col(if done { "#9ca3af" } else { "#0f1115" })),
        strikethrough: Some(done),
        ..Default::default()
    })
}

fn tag_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(11.0)),
        color: Some(col("#6b7280")),
        ..Default::default()
    })
}
