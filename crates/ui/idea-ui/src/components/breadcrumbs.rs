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

use runtime_core::{component, ui, IdealystSchema, IntoElement, Element, Reactive, StyleApplication};

use crate::stylesheets::{BreadcrumbItem, BreadcrumbRow, BreadcrumbSeparator};

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
    /// Separator glyph between crumbs. Default `/`.
    pub separator: String,
}

impl Default for BreadcrumbsProps {
    fn default() -> Self {
        Self { items: Vec::new(), separator: "/".to_string() }
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

    let mut kids: Vec<Element> = Vec::with_capacity(n * 2);
    for (i, crumb) in props.items.into_iter().enumerate() {
        let is_current = i + 1 == n;
        let item: Element = match crumb.on_press {
            Some(cb) if !is_current => runtime_core::pressable(
                vec![runtime_core::text(crumb.label).into_element()],
                move || (cb)(),
            )
            .with_style(crumb_style(false))
            .into_element(),
            _ => runtime_core::text(crumb.label)
                .with_style(crumb_style(is_current))
                .into_element(),
        };
        kids.push(item);
        if !is_current {
            kids.push(
                runtime_core::text(sep.clone())
                    .with_style(BreadcrumbSeparator())
                    .into_element(),
            );
        }
    }

    ui! { view(style = BreadcrumbRow()) { kids } }
}
