//! `Divider` — thin separator line, horizontal or vertical.

use framework_core::{ui, Primitive};

use crate::stylesheets::Divider;
pub use crate::stylesheets::DividerAxis;

#[derive(Default)]
pub struct DividerProps {
    pub axis: DividerAxis,
}

pub fn divider(props: &DividerProps) -> Primitive {
    let axis = props.axis;
    let style = Divider().axis(axis);
    ui! { View(style = style) {} }
}
