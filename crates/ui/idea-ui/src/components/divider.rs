//! `Divider` — thin separator line, horizontal or vertical.

use runtime_core::{component, ui, Element, IdealystSchema};

use crate::stylesheets::Divider as DividerStyle;
pub use crate::stylesheets::DividerAxis;

#[derive(Default, IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct DividerProps {
    /// Orientation of the separator line. Horizontal spans the parent's
    /// width (1px tall); Vertical spans its height (1px wide).
    pub axis: DividerAxis,
}

/// Thin themed separator line. Renders as a 1px rule along the chosen
/// `axis`, picking up the theme's border color.
#[component]
pub fn Divider(props: &DividerProps) -> Element {
    let axis = props.axis;
    let style = DividerStyle().axis(axis);
    ui! { view(style = style) {} }
}
