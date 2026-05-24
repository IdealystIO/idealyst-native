//! `Stack` — an opinionated flex container.
//!
//! Wraps a `View` with the [`Stack`](crate::stylesheets::Stack) stylesheet
//! pre-applied. One component covers both column and row layouts via the
//! `axis` prop; the default is `Column` (the common case for screens and
//! card bodies). Use `axis = StackAxis::Row` for row layouts (toolbars,
//! button groups, badge rows).

use runtime_core::{ui, ChildList, Primitive};

use crate::stylesheets::Stack;

// Re-export the stylesheet-generated variant enums.
pub use crate::stylesheets::{StackAlign, StackAxis, StackGap, StackJustify, StackPadding};

#[derive(Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct StackProps {
    pub gap: StackGap,
    /// Token-driven inner padding. Defaults to `None` so a Stack
    /// without an explicit `padding` prop matches its pre-padding
    /// behaviour. Sizes track the theme's spacing scale, same as
    /// `gap` — pick `Xs`/`Sm`/`Md`/`Lg`/`Xl` and the value comes
    /// from `t.spacing()` so it reflects the active theme.
    pub padding: StackPadding,
    pub axis: StackAxis,
    pub align: StackAlign,
    pub justify: StackJustify,
    pub children: Vec<Primitive>,
}

pub fn stack(props: StackProps) -> Primitive {
    let style = Stack()
        .gap(props.gap)
        .padding(props.padding)
        .axis(props.axis)
        .align(props.align)
        .justify(props.justify);
    let mut children: Vec<Primitive> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { View(style = style) { children } }
}
