//! `Center` — a container that centers its children on both axes.
//!
//! Equivalent to a `View` with `align_items: center` +
//! `justify_content: center`. Mostly exists so the common case
//! (centering a loading spinner, an empty-state illustration, etc.)
//! doesn't need a one-off stylesheet.

use runtime_core::{component, ui, ChildList, Element, IdealystSchema};

use crate::stylesheets::Center as CenterStyle;

#[derive(Default)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct CenterProps {
    /// Children to center on both axes. Incoming fragments are flattened
    /// into the centered container.
    pub children: Vec<Element>,
}

/// Renders a container that centers its children on both axes (a `view`
/// with `align_items: center` + `justify_content: center`).
#[component(children)]
pub fn Center(props: CenterProps) -> Element {
    let style = CenterStyle();
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { view(style = style) { children } }
}
