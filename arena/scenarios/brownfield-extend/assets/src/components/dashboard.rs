//! `Dashboard` — the top-level layout that composes the panels.

use runtime_core::{component, ui, Element, StyleRules, StyleSheet};
use std::rc::Rc;

use crate::components::header::Header;
use crate::components::task_list::TaskList;
use crate::components::toolbar::Toolbar;
use crate::store::TaskStore;
use crate::style_helpers::{col, column, static_sheet};

#[derive(Default)]
pub struct DashboardProps {
    /// The shared store, threaded down to every panel.
    pub store: TaskStore,
}

/// Root view: stacks the [`Header`], [`Toolbar`], and [`TaskList`] in a
/// padded page and hands each the same [`TaskStore`]. Because all three
/// share one store, filter changes made in the Toolbar surface in the
/// Header's counts and the TaskList's rows with no cross-wiring.
#[component]
pub fn Dashboard(props: &DashboardProps) -> Element {
    let store = props.store;
    ui! {
        view(style = page_sheet()) {
            view(style = column(12.0, 16.0)) {
                Header(store = store)
                Toolbar(store = store)
                TaskList(store = store)
            }
        }
    }
}

fn page_sheet() -> Rc<StyleSheet> {
    use runtime_core::{Length, Tokenized};
    static_sheet(StyleRules {
        width: Some(Tokenized::Literal(Length::Percent(100.0))),
        height: Some(Tokenized::Literal(Length::Percent(100.0))),
        background: Some(col("#f7f5ef")),
        ..Default::default()
    })
}
