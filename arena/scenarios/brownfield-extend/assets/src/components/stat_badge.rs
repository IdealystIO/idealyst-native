//! `StatBadge` — a labelled metric shown in the Header.

use runtime_core::{
    component, ui, Element, FontWeight, IdealystSchema, Reactive, StyleRules, StyleSheet,
};
use std::rc::Rc;

use crate::style_helpers::{col, px, static_sheet};

/// Props for [`StatBadge`].
#[derive(Default, IdealystSchema)]
pub struct StatBadgeProps {
    /// The metric's caption (e.g. "Total", "Done").
    pub label: String,
    /// The metric's value. Pass a live `Reactive` (e.g. `rx!(...)`) so the
    /// badge repaints when the underlying count changes.
    pub value: Reactive<String>,
}

/// A small caption-over-value pair used to surface a single derived
/// number in the Header (total tasks, completed count, visible count).
#[component]
pub fn StatBadge(props: &StatBadgeProps) -> Element {
    let label = props.label.clone();
    let value = props.value.clone();
    ui! {
        view(style = wrapper_sheet()) {
            text(style = value_sheet()) { value }
            text(style = label_sheet()) { label }
        }
    }
}

fn wrapper_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        gap: Some(px(2.0)),
        ..Default::default()
    })
}

fn value_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(22.0)),
        font_weight: Some(FontWeight::Bold),
        color: Some(col("#0f1115")),
        ..Default::default()
    })
}

fn label_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(12.0)),
        color: Some(col("#6b7280")),
        ..Default::default()
    })
}
