//! `List` / `ListItem` — a bordered, divided vertical list of rows.
//!
//! ```ignore
//! ui! {
//!     List {
//!         ListItem(label = "Profile", on_press = on_profile)
//!         ListItem(label = "Billing", on_press = on_billing, trailing = Some(badge))
//!         ListItem(label = "Sign out", on_press = on_signout)
//!     }
//! }
//! ```
//!
//! `List` draws the surrounding surface + border and inserts a hairline
//! divider between consecutive items (none before the first). Each
//! `ListItem` is a row with optional leading/trailing content; passing
//! `on_press` makes the whole row clickable with a hover highlight.

use std::rc::Rc;

use runtime_core::{
    component, ui, ChildList, IdealystSchema, IntoElement, Element, Reactive, StyleApplication,
};

use crate::stylesheets::{Divider, ListContainer, ListItemRow};

// =============================================================================
// List
// =============================================================================

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct ListProps {
    /// The rows (typically [`ListItem`]s); hairline dividers are
    /// inserted between consecutive rows.
    pub children: Vec<Element>,
}

impl Default for ListProps {
    fn default() -> Self {
        Self { children: Vec::new() }
    }
}

/// Renders a bordered surface wrapping its rows, with a hairline
/// divider between each consecutive pair.
#[component(children)]
pub fn List(props: ListProps) -> Element {
    let mut items: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut items);
    }

    // Interleave a hairline divider between consecutive rows.
    let mut kids: Vec<Element> = Vec::with_capacity(items.len() * 2);
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            kids.push(ui! { view(style = Divider()) {} });
        }
        kids.push(item);
    }

    ui! { view(style = ListContainer()) { kids } }
}

// =============================================================================
// ListItem
// =============================================================================

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct ListItemProps {
    /// Row label. `Reactive<String>` — static or live.
    pub label: Reactive<String>,
    /// When `Some`, the row is clickable (hover highlight + handler).
    pub on_press: Option<Rc<dyn Fn()>>,
    /// Optional leading element (icon, avatar).
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub leading: Option<Element>,
    /// Optional trailing element (badge, chevron), pushed to the right.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub trailing: Option<Element>,
    /// Render the row in its highlighted/selected state.
    pub active: bool,
}

impl Default for ListItemProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            on_press: None,
            leading: None,
            trailing: None,
            active: false,
        }
    }
}

fn spacer() -> Element {
    runtime_core::view(Vec::new())
        .with_style(|| StyleApplication::new(crate::stylesheets::Spacer::sheet()))
        .into_element()
}

/// A single list row: optional leading element, label, and a trailing
/// element pushed to the right edge. With `on_press` set the whole row
/// is tappable with a hover highlight.
#[component]
pub fn ListItem(props: ListItemProps) -> Element {
    let active = props.active;

    let mut kids: Vec<Element> = Vec::with_capacity(4);
    if let Some(l) = props.leading {
        kids.push(l);
    }
    kids.push(runtime_core::text(props.label).into_element());
    if let Some(tr) = props.trailing {
        kids.push(spacer());
        kids.push(tr);
    }

    let style = move || {
        StyleApplication::new(ListItemRow::sheet())
            .with("active", if active { "on" } else { "off" }.to_string())
    };

    match props.on_press {
        Some(cb) => runtime_core::pressable(kids, move || (cb)())
            .with_style(style)
            .into_element(),
        None => runtime_core::view(kids).with_style(style).into_element(),
    }
}
