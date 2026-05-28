//! `Center` — a container that centers its children on both axes.
//!
//! Equivalent to a `View` with `align_items: center` +
//! `justify_content: center`. Mostly exists so the common case
//! (centering a loading spinner, an empty-state illustration, etc.)
//! doesn't need a one-off stylesheet.

use runtime_core::{ui, ChildList, Element};

use crate::stylesheets::Center;

#[derive(Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct CenterProps {
    pub children: Vec<Element>,
}

pub fn center(props: CenterProps) -> Element {
    let style = Center();
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { View(style = style) { children } }
}
