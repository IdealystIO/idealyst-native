//! `Header` — the dashboard's title bar with derived stat badges.

use runtime_core::{component, rx, ui, Element, FontWeight, StyleRules, StyleSheet};
use std::rc::Rc;

use crate::components::card::Card;
use crate::components::stat_badge::StatBadge;
use crate::store::TaskStore;
use crate::style_helpers::{col, px, row, static_sheet};

#[derive(Default)]
pub struct HeaderProps {
    /// The shared store the badges derive their counts from.
    pub store: TaskStore,
}

/// Title bar showing the app name and three live [`StatBadge`]s derived
/// from the [`TaskStore`]: total tasks, completed count, and how many are
/// currently visible under the active filters. The visible badge reads
/// `store.visible_count()`, so it tracks whatever filtering the store
/// applies without the Header knowing the filter rules.
#[component]
pub fn Header(props: &HeaderProps) -> Element {
    let store = props.store;
    ui! {
        Card() {
            view(style = row(12.0)) {
                text(style = title_sheet()) { "Task Dashboard" }
                view(style = stats_row()) {
                    StatBadge(label = "Total", value = rx!(format!("{}", store.total())))
                    StatBadge(label = "Done", value = rx!(format!("{}", store.completed())))
                    StatBadge(label = "Showing", value = rx!(format!("{}", store.visible_count())))
                }
            }
        }
    }
}

fn title_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(20.0)),
        font_weight: Some(FontWeight::Bold),
        color: Some(col("#0f1115")),
        ..Default::default()
    })
}

fn stats_row() -> Rc<StyleSheet> {
    use runtime_core::{AlignItems, FlexDirection};
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: Some(px(20.0)),
        ..Default::default()
    })
}
