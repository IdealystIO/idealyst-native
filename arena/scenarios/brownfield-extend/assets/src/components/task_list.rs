//! `TaskList` — renders the store's visible tasks as keyed rows.

use runtime_core::{component, ui, Element, StyleRules, StyleSheet};
use std::rc::Rc;

use crate::components::task_row::TaskRow;
use crate::store::TaskStore;
use crate::style_helpers::{px, static_sheet};

#[derive(Default)]
pub struct TaskListProps {
    /// The shared store. The list binds to `store.visible`, so it shows
    /// exactly the rows the store's filters admit.
    pub store: TaskStore,
}

/// The scrollable body of the dashboard. Iterates `store.visible` (the
/// store's derived, filtered view) with a keyed `for`, emitting one
/// [`TaskRow`] per task. Because it reads the derived view rather than
/// filtering itself, any filter the store applies is reflected here with
/// no change to this component.
#[component]
pub fn TaskList(props: &TaskListProps) -> Element {
    let store = props.store;
    let visible = store.visible;
    ui! {
        view(style = list_sheet()) {
            for task in visible, key = task.id {
                TaskRow(store = store, task = task.clone())
            }
        }
    }
}

fn list_sheet() -> Rc<StyleSheet> {
    use runtime_core::FlexDirection;
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        gap: Some(px(8.0)),
        ..Default::default()
    })
}
