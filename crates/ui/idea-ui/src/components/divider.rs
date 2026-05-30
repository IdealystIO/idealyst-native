//! `Divider` — thin separator line, horizontal or vertical.

use runtime_core::{component, ui, Element};

use crate::stylesheets::Divider as DividerStyle;
pub use crate::stylesheets::DividerAxis;

#[derive(Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct DividerProps {
    pub axis: DividerAxis,
}

#[component]
pub fn Divider(props: &DividerProps) -> Element {
    let axis = props.axis;
    let style = DividerStyle().axis(axis);
    ui! { View(style = style) {} }
}
