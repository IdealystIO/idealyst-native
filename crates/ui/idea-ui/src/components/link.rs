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

use runtime_core::{component, IntoElement, Element, Reactive};

use crate::stylesheets::LinkText;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct LinkProps {
    /// Link text. `Reactive<String>` — static or live.
    pub label: Reactive<String>,
    /// Destination URL (`https:`, `mailto:`, `tel:`, …).
    pub url: String,
}

impl Default for LinkProps {
    fn default() -> Self {
        Self { label: Reactive::Static(String::new()), url: String::new() }
    }
}

#[component]
pub fn Link(props: &LinkProps) -> Element {
    let text = runtime_core::text(props.label.clone())
        .with_style(LinkText())
        .into_element();
    runtime_core::external_link(props.url.clone(), vec![text]).into_element()
}
