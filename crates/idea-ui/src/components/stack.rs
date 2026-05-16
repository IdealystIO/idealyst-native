//! `Stack` / `HStack` / `VStack` — opinionated flex containers.
//!
//! Wrap a `View` with the [`Stack`](crate::stylesheets::Stack) stylesheet
//! pre-applied. `HStack` / `VStack` are tiny aliases that lock the axis.

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

#[derive(Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct VStackProps {
    pub gap: StackGap,
    pub align: StackAlign,
    pub justify: StackJustify,
    pub children: Vec<Primitive>,
}

pub fn vstack(props: VStackProps) -> Primitive {
    let style = Stack()
        .gap(props.gap)
        .axis(StackAxis::Column)
        .align(props.align)
        .justify(props.justify);
    let mut children: Vec<Primitive> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { View(style = style) { children } }
}

#[derive(Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct HStackProps {
    pub gap: StackGap,
    pub align: StackAlign,
    pub justify: StackJustify,
    pub children: Vec<Primitive>,
}

pub fn hstack(props: HStackProps) -> Primitive {
    let style = Stack()
        .gap(props.gap)
        .axis(StackAxis::Row)
        .align(props.align)
        .justify(props.justify);
    let mut children: Vec<Primitive> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { View(style = style) { children } }
}
