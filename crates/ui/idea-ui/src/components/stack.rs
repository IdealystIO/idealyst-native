//! `Stack` — an opinionated flex container.
//!
//! Wraps a `View` with the [`Stack`](crate::stylesheets::Stack) stylesheet
//! pre-applied. One component covers both column and row layouts via the
//! `axis` prop; the default is `Column` (the common case for screens and
//! card bodies). Use `axis = StackAxis::Row` for row layouts (toolbars,
//! button groups, badge rows).

use framework_core::{ui, ChildList, Primitive};

use crate::stylesheets::Stack;

// Re-export the stylesheet-generated variant enums.
pub use crate::stylesheets::{StackAlign, StackAxis, StackGap, StackJustify};

#[derive(Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct StackProps {
    pub gap: StackGap,
    pub axis: StackAxis,
    pub align: StackAlign,
    pub justify: StackJustify,
    pub children: Vec<Primitive>,
}

pub fn stack(props: StackProps) -> Primitive {
    let style = Stack()
        .gap(props.gap)
        .axis(props.axis)
        .align(props.align)
        .justify(props.justify);
    let mut children: Vec<Primitive> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { View(style = style) { children } }
}
