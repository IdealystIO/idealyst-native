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
    component, component_styles, ui, AlignItems, ChildList, Color, Cursor, Element, FlexDirection,
    IdealystSchema, IntoElement, Reactive, TextAlign,
};

use crate::components::divider::DividerStyles;
use crate::theme::IdeaThemeRef;

// =============================================================================
// List
// =============================================================================

component_styles! {
    /// The styled sections of [`List`].
    pub List<IdeaThemeRef> {
        slot root {
            base(t) {
                flex_direction: FlexDirection::Column,
                background: t.color.surface(),
                border_top_width: 1.0,
                border_right_width: 1.0,
                border_bottom_width: 1.0,
                border_left_width: 1.0,
                border_top_color: t.color.border(),
                border_right_color: t.color.border(),
                border_bottom_color: t.color.border(),
                border_left_color: t.color.border(),
                border_top_left_radius: t.radius.lg(),
                border_top_right_radius: t.radius.lg(),
                border_bottom_left_radius: t.radius.lg(),
                border_bottom_right_radius: t.radius.lg(),
                overflow: runtime_core::Overflow::Hidden,
            }
        }
    }
}

// Reactive-by-default: only field is `children` (a LIST, auto-skipped);
// `#[props]` is a no-op here but kept for uniformity with the family.
#[runtime_core::props(styles = ListStyles)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct ListProps {
    /// The rows (typically [`ListItem`]s); hairline dividers are
    /// inserted between consecutive rows.
    pub children: Vec<Element>,
}

impl Default for ListProps {
    fn default() -> Self {
        Self { children: Vec::new(), styles: Default::default() }
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
            kids.push(ui! { view(style = DividerStyles::root()) {} });
        }
        kids.push(item);
    }

    let root_style = ListStyles::root().overridden(props.styles.root.as_ref());
    ui! { view(style = root_style) { kids } }
}

// =============================================================================
// ListItem
// =============================================================================

component_styles! {
    /// The styled sections of [`ListItem`].
    pub ListItem<IdeaThemeRef> {
        slot root {
            base(t) {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: t.spacing.sm(),
                padding_vertical: t.spacing.sm(),
                padding_horizontal: t.spacing.md(),
                background: Color("transparent".into()),
                color: t.color.text(),
                font_size: t.typography.body_size(),
                text_align: TextAlign::Left,
            }
            variant active {
                #[default]
                off(_t) {}
                on(t) { background: t.color.surface_alt() }
            }
            // A row with an `on_press` shows a pointer (the "anything pressable
            // shows a pointer" rule). A variant arm rather than a computed layer
            // so the rule premints — a constant closure defeats premint for the
            // whole sheet without expressing anything a variant can't.
            variant interactive {
                #[default]
                off(_t) {}
                on(_t) { cursor: Cursor::Pointer }
            }
            state hovered(t) { background: t.color.surface_alt() }
            // Focus mirrors hover (row wash) — replaces the native macOS ring.
            state focused(t) { background: t.color.surface_alt() }
            transitions { background: 120ms EaseOut }
        }
    }
}

// Reactive-by-default: `#[props]` wraps the scalar `active` → `Reactive<bool>`
// (routes to the row style sink below). `label` is already reactive; the
// `on_press` handler and `leading`/`trailing` element slots auto-skip.
#[runtime_core::props(styles = ListItemStyles)]
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
            styles: Default::default(),
        }
    }
}

fn spacer() -> Element {
    runtime_core::view(Vec::new())
        .with_style(crate::components::spacer::SpacerStyles::root())
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

    // Clickable rows show a pointer (the "anything pressable shows a pointer"
    // rule); static rows don't. `on_press` is known at build time, so the
    // `interactive` arm is fixed per row; `active` may be live and re-styles
    // the row in place.
    let style = ListItemStyles::root()
        .set(active.map(ListItemActive::from))
        .set(ListItemInteractive::from(props.on_press.is_some()))
        .overridden(props.styles.root.as_ref());

    match props.on_press {
        Some(cb) => runtime_core::pressable(kids, move || (cb)()).with_style(style).into_element(),
        None => runtime_core::view(kids).with_style(style).into_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::with_test_world;
    use runtime_core::Cursor;

    fn install() {
        use idea_theme::theme::{install_idea_theme, light_theme};
        install_idea_theme(light_theme());
    }

    /// The pointer cursor on a pressable row rides the sheet's `interactive`
    /// variant (not a `with_computed` layer) so the rule premints. Pins the
    /// observable behavior: pressable rows get a pointer, plain rows keep
    /// the default.
    #[test]
    fn regression_pressable_list_item_keeps_pointer_cursor() {
        with_test_world(|| {
            install();
            let el = ListItem(ListItemProps {
                label: Reactive::Static("Row".to_string()),
                on_press: Some(std::rc::Rc::new(|| {})),
                ..Default::default()
            });
            let style = match classify(el) {
                P::Pressable { style, .. } => style.expect("pressable row carries a style"),
                _ => panic!("a row with on_press builds a pressable"),
            };
            assert_eq!(style.resolve().cursor, Some(Cursor::Pointer));
        });
    }

    #[test]
    fn regression_inert_list_item_has_no_pointer_cursor() {
        with_test_world(|| {
            install();
            let el = ListItem(ListItemProps {
                label: Reactive::Static("Row".to_string()),
                ..Default::default()
            });
            let style = match classify(el) {
                P::View { style, .. } => style.expect("row carries a style"),
                _ => panic!("a row without on_press builds a view"),
            };
            assert_ne!(style.resolve().cursor, Some(Cursor::Pointer));
        });
    }
}
