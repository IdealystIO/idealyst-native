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
    component, ui, ChildList, Cursor, Element, IdealystSchema, IntoElement, Reactive,
    StyleApplication, StyleRules,
};

use crate::stylesheets::{Divider, ListContainer, ListItemRow};

// =============================================================================
// List
// =============================================================================

// Reactive-by-default: only field is `children` (a LIST, auto-skipped);
// `#[props]` is a no-op here but kept for uniformity with the family.
#[runtime_core::props]
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

// Reactive-by-default: `#[props]` wraps the scalar `active` → `Reactive<bool>`
// (routes to the row style sink below). `label` is already reactive; the
// `on_press` handler and `leading`/`trailing` element slots auto-skip.
#[runtime_core::props]
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
    /// Render the row in its highlighted/selected state. `Reactive<bool>` —
    /// static or live (signal/`rx!`); the row re-styles in place.
    pub active: bool,
}

impl Default for ListItemProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            on_press: None,
            leading: None,
            trailing: None,
            active: Reactive::Static(false),
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
    let active = props.active.clone();

    let mut kids: Vec<Element> = Vec::with_capacity(4);
    if let Some(l) = props.leading {
        kids.push(l);
    }
    kids.push(runtime_core::text(props.label).into_element());
    if let Some(tr) = props.trailing {
        kids.push(spacer());
        kids.push(tr);
    }

    // `active` is read live INSIDE each style closure so the apply-style Effect
    // subscribes when it's a signal; a static stays the build-time fast path.
    let style_is_reactive = !active.is_static();

    match props.on_press {
        // Clickable row: pressable + a pointer cursor so it reads as
        // interactive (the "anything pressable shows a pointer" rule). The
        // cursor rides the single computed slot alongside the active variant.
        Some(cb) => {
            let make_style = move || {
                StyleApplication::new(ListItemRow::sheet())
                    .with("active", if active.get() { "on" } else { "off" }.to_string())
                    .with_computed("li-cursor", || StyleRules {
                        cursor: Some(Cursor::Pointer),
                        ..Default::default()
                    })
            };
            let bound = runtime_core::pressable(kids, move || (cb)());
            if style_is_reactive {
                bound.with_style(make_style).into_element()
            } else {
                bound.with_style(make_style()).into_element()
            }
        }
        // Static row: no pointer (it isn't interactive).
        None => {
            let make_style = move || {
                StyleApplication::new(ListItemRow::sheet())
                    .with("active", if active.get() { "on" } else { "off" }.to_string())
            };
            let bound = runtime_core::view(kids);
            if style_is_reactive {
                bound.with_style(make_style).into_element()
            } else {
                bound.with_style(make_style()).into_element()
            }
        }
    }
}
