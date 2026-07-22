//! `Toolbar` — filter controls for the dashboard.

use runtime_core::{component, ui, Element, FontWeight, StyleRules, StyleSheet};
use std::rc::Rc;

use crate::components::card::Card;
use crate::store::{Priority, TaskStore};
use crate::style_helpers::{col, px, static_sheet};

#[derive(Default)]
pub struct ToolbarProps {
    /// The shared store the controls drive.
    pub store: TaskStore,
}

/// The dashboard's control strip. Its buttons set the store's priority
/// filter (All / High / Normal / Low); the store recomputes its visible
/// view and every reader updates. New filter controls belong here, wired
/// to the store the same way.
#[component]
pub fn Toolbar(props: &ToolbarProps) -> Element {
    let store = props.store;
    ui! {
        Card() {
            view(style = bar_sheet()) {
                text(style = label_sheet()) { "Priority:" }
                button(label = "All", on_click = move || store.set_priority_filter(None))
                button(
                    label = "High",
                    on_click = move || store.set_priority_filter(Some(Priority::High)),
                )
                button(
                    label = "Normal",
                    on_click = move || store.set_priority_filter(Some(Priority::Normal)),
                )
                button(
                    label = "Low",
                    on_click = move || store.set_priority_filter(Some(Priority::Low)),
                )
            }
        }
    }
}

fn bar_sheet() -> Rc<StyleSheet> {
    use runtime_core::{AlignItems, FlexDirection};
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: Some(px(8.0)),
        ..Default::default()
    })
}

fn label_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(13.0)),
        font_weight: Some(FontWeight::Medium),
        color: Some(col("#6b7280")),
        ..Default::default()
    })
}
