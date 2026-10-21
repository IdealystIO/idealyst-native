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

use runtime_core::{component, component_styles, Cursor, Element, FontWeight, IdealystSchema, IntoElement, Reactive};

use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Link`].
    pub Link<IdeaThemeRef> {
        slot text {
            base(t) {
                color: t.intent.primary.fg(),
                font_size: t.typography.body_size(),
                font_weight: FontWeight::Medium,
                // Web gets this free from the UA stylesheet (`a[href]` is
                // `cursor: pointer`); no native backend has an equivalent, and
                // the framework imposes no default cursor on any primitive. So
                // without declaring it the SAME author code shows a hand on web
                // and an arrow everywhere else — the divergence CLAUDE.md §7
                // exists to prevent. Declaring it in the sheet converges them
                // and stays overridable.
                cursor: Cursor::Pointer,
            }
            state hovered(t) { color: t.intent.primary.solid_bg() }
            // Focus mirrors hover (color brighten) — an inline text link takes a
            // text affordance, not a box ring. Replaces the native macOS ring.
            state focused(t) { color: t.intent.primary.solid_bg() }
            transitions { color: 120ms EaseOut }
        }
    }
}

// Reactive-by-default: `#[props]` wraps `url` → `Reactive<String>`; `label` is
// already reactive. `label` routes to the `text()` sink (live); a live `url`
// routes to the `external_link` reactive `.url()` setter so the href swaps in
// place (a `Static` url is set once at construction, no effect).
#[runtime_core::props(styles = LinkStyles)]
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
            styles: Default::default(),
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
        .with_style(LinkStyles::text().overridden(props.styles.text.as_ref()))
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

#[cfg(test)]
mod tests {
    use super::LinkStyles;
    use idea_theme::testing::with_test_world;
    use runtime_core::{resolve_style, Cursor};

    /// A link must show the pointer affordance on EVERY backend.
    ///
    /// Web gets it free from the UA stylesheet (`a[href]` is
    /// `cursor: pointer`); no native backend has an equivalent, and the
    /// framework deliberately imposes no default cursor on any primitive.
    /// So an undeclared cursor here means identical author code renders a
    /// hand on web and a plain arrow on GTK/AppKit — exactly the
    /// cross-platform divergence CLAUDE.md §7 forbids, and exactly how it
    /// was reported ("cursor changes don't work" on Linux, while the same
    /// docs site looked right in a browser).
    #[test]
    fn regression_link_declares_the_pointer_cursor_for_native_backends() {
        with_test_world(|| {
            idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
            let rules = resolve_style(&LinkStyles::text().into_style_application());
            assert_eq!(
                rules.cursor,
                Some(Cursor::Pointer),
                "the link text slot must declare `cursor: Pointer`. Web would still look \
                 correct without it (the UA gives `<a href>` a hand), which is \
                 what let this hide — native backends show an arrow.",
            );
        });
    }
}
