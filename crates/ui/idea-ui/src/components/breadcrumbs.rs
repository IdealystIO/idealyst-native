//! `Breadcrumbs` — a horizontal trail of navigational links ending in
//! the current page.
//!
//! ```ignore
//! ui! {
//!     Breadcrumbs(items = vec![
//!         Crumb::linked("Home", Rc::new(move || go_home())),
//!         Crumb::linked("Library", Rc::new(move || go_library())),
//!         Crumb::new("Settings"), // last = current, not clickable
//!     ])
//! }
//! ```
//!
//! The last crumb renders as the current page (bold, no link);
//! every earlier crumb with an `on_press` is clickable.

use std::rc::Rc;

use runtime_core::{
    component, icon, ui, Color, Cursor, Element, IconData, IdealystSchema, IntoElement, Reactive,
    StyleApplication, StyleRules, Tokenized,
};

use crate::stylesheets::{BreadcrumbItem, BreadcrumbRow, BreadcrumbSeparator};

/// Pixel size of an icon separator — matches the crumb text's `body-sm`
/// line so the glyph sits on the same baseline as the labels.
const SEPARATOR_ICON_PX: f32 = 14.0;

/// One crumb. `Crumb::new(label)` is a plain (non-clickable) crumb;
/// `Crumb::linked(label, on_press)` is clickable.
#[derive(Clone, IdealystSchema)]
pub struct Crumb {
    /// Crumb text. `Reactive<String>` — static or live (signal/`rx!`).
    pub label: Reactive<String>,
    /// When `Some` (and not the last crumb), the crumb is a clickable
    /// link firing this handler.
    pub on_press: Option<Rc<dyn Fn()>>,
}

impl Crumb {
    pub fn new(label: impl Into<Reactive<String>>) -> Self {
        Self { label: label.into(), on_press: None }
    }
    pub fn linked(label: impl Into<Reactive<String>>, on_press: Rc<dyn Fn()>) -> Self {
        Self { label: label.into(), on_press: Some(on_press) }
    }
}

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct BreadcrumbsProps {
    /// The trail, root-first. The last item renders as the current page
    /// (emphasized, not clickable).
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub items: Vec<Crumb>,
    /// Separator glyph between crumbs. Default `/`. Ignored when
    /// [`separator_icon`](Self::separator_icon) is `Some`.
    pub separator: String,
    /// Optional icon separator (e.g. `icons_lucide::CHEVRON_RIGHT`). When
    /// `Some`, an icon is drawn between crumbs instead of the text glyph;
    /// `None` falls back to the [`separator`](Self::separator) string — so
    /// both forms are supported.
    pub separator_icon: Option<IconData>,
}

impl Default for BreadcrumbsProps {
    fn default() -> Self {
        Self { items: Vec::new(), separator: "/".to_string(), separator_icon: None }
    }
}

fn crumb_style(is_current: bool) -> impl Fn() -> StyleApplication + Clone + 'static {
    move || {
        StyleApplication::new(BreadcrumbItem::sheet())
            .with("current", if is_current { "on" } else { "off" }.to_string())
    }
}

/// Renders a horizontal trail of crumbs joined by `separator`. Earlier
/// linked crumbs are clickable; the last crumb is the current page.
#[component]
pub fn Breadcrumbs(props: BreadcrumbsProps) -> Element {
    let n = props.items.len();
    let sep = props.separator;
    let sep_icon = props.separator_icon;

    let mut kids: Vec<Element> = Vec::with_capacity(n * 2);
    for (i, crumb) in props.items.into_iter().enumerate() {
        let is_current = i + 1 == n;
        let item: Element = match crumb.on_press {
            // A clickable crumb: pressable + a pointer cursor so it reads as
            // interactive (the current/last crumb and plain crumbs stay
            // default-cursor text).
            Some(cb) if !is_current => runtime_core::pressable(
                vec![runtime_core::text(crumb.label).into_element()],
                move || (cb)(),
            )
            .with_style(|| {
                crumb_style(false)().with_computed("bc-link-cursor", || StyleRules {
                    cursor: Some(Cursor::Pointer),
                    ..Default::default()
                })
            })
            .into_element(),
            _ => runtime_core::text(crumb.label)
                .with_style(crumb_style(is_current))
                .into_element(),
        };
        kids.push(item);
        if !is_current {
            // Icon separator when provided, else the text glyph — both forms
            // are supported. The icon is tinted muted to match the glyph.
            let sep_el = match sep_icon {
                Some(data) => icon(data)
                    .size(SEPARATOR_ICON_PX)
                    .color(|| {
                        Tokenized::token("color-text-muted", Color("#6b7280".into())).resolve()
                    })
                    .into_element(),
                None => runtime_core::text(sep.clone())
                    .with_style(BreadcrumbSeparator())
                    .into_element(),
            };
            kids.push(sep_el);
        }
    }

    ui! { view(style = BreadcrumbRow()) { kids } }
}
