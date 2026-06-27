//! `Link` — a styled external/inline navigational link.
//!
//! ```ignore
//! ui! { Link(label = "Read the docs", url = "https://example.com/docs") }
//! ```
//!
//! `ui!` routes the PascalCase `Link` tag to this component; the
//! lowercase `link` tag is the framework's in-app routing primitive.
//!
//! Wraps the framework's `external_link` primitive: on web it renders a
//! real `<a href target="_blank" rel="noopener">`; on native it hands
//! the URL to the platform opener. For *in-app* route navigation, use
//! the framework's `link(&route, params, children)` primitive directly
//! — that needs a typed `Route`, which is app-specific and out of scope
//! for a generic UI component.

use runtime_core::{component, IdealystSchema, IntoElement, Element, Reactive};

use crate::stylesheets::LinkText;

// Reactive-by-default: `#[props]` wraps `url` → `Reactive<String>`; `label` is
// already reactive. `label` routes to the `text()` sink (live); a live `url`
// routes to the `external_link` reactive `.url()` setter so the href swaps in
// place (a `Static` url is set once at construction, no effect).
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct LinkProps {
    /// Link text. `Reactive<String>` — static or live.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
    /// Destination URL (`https:`, `mailto:`, `tel:`, …).
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub url: Reactive<String>,
}

impl Default for LinkProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            url: Reactive::Static(String::new()),
        }
    }
}

/// Renders a styled external/inline navigational link: a styled text node
/// wrapped in the framework's `external_link` primitive (a real `<a>` on
/// web, the platform URL opener on native).
#[component]
pub fn Link(props: &LinkProps) -> Element {
    // `label` routes live to the `text()` sink — a `Signal`/`rx!` re-renders
    // the link text in place.
    let text = runtime_core::text(props.label.clone())
        .with_style(LinkText())
        .into_element();
    // A live `url` routes to the primitive's reactive `.url()` setter (swaps the
    // `<a href>` in place); a `Static` url just seeds `external_link` once.
    let mut node = runtime_core::external_link(props.url.get(), vec![text]);
    if !props.url.is_static() {
        let url = props.url.clone();
        node = node.url(move || url.get());
    }
    node.into_element()
}
