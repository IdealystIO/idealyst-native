//! `Center` — a container that centers its children on both axes.
//!
//! Equivalent to a `View` with `align_items: center` +
//! `justify_content: center`. Mostly exists so the common case
//! (centering a loading spinner, an empty-state illustration, etc.)
//! doesn't need a one-off stylesheet.

use runtime_core::{component, component_styles, ui, AlignItems, ChildList, Element, FlexDirection, IdealystSchema, JustifyContent};

use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Center`].
    pub Center<IdeaThemeRef> {
        slot root {
            base(_t) {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
            }
        }
    }
}

// Reactive-by-default: `#[props]` wraps scalar-DATA fields → `Reactive<T>`.
// `Center` has only `children` (the children category — left bare), so this is
// a no-op for reactivity; the attribute keeps the component uniform with its
// siblings and ready for future data props.
#[runtime_core::props(styles = CenterStyles)]
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
    let style = CenterStyles::root().overridden(props.styles.root.as_ref());
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { view(style = style) { children } }
}
