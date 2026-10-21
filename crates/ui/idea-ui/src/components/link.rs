//! `Link` — a styled link: to a URL, or to an in-app action.
//!
//! ```ignore
//! ui! { Link(label = "Read the docs", url = "https://example.com/docs") }
//! ui! { Link(label = "Show 1 archived", on_press = Some(show_archived)) }
//! ```
//!
//! `ui!` routes the PascalCase `Link` tag to this component; the
//! lowercase `link` tag is the framework's in-app routing primitive.
//!
//! With a `url`, Link wraps the framework's `external_link` primitive: on
//! web a real `<a href target="_blank" rel="noopener">`, on native the
//! platform URL opener.
//!
//! With `on_press`, Link is an in-app ACTION drawn as link text
//! (underlined, in the text color): it runs the callback and goes nowhere.
//! It is a `pressable` — a button to assistive tech (an action is not
//! navigation, so it must not announce as a link or offer "open in new
//! tab"), Tab-focusable and activated by Enter / Space on web, a native
//! press target elsewhere. `on_press` wins over `url` when both are set.
//!
//! For *in-app* route navigation, use the framework's
//! `link(&route, params, children)` primitive directly — that needs a typed
//! `Route`, which is app-specific and out of scope for a generic UI
//! component.

use std::rc::Rc;

use runtime_core::{component, component_styles, Cursor, Element, FontWeight, IdealystSchema, IntoElement, Reactive};


use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Link`].
    pub Link<IdeaThemeRef> {
        /// The link text.
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
            /// `on`: an in-app action (`on_press`) — underlined text in
            /// the body text color, so it reads as a link inside prose
            /// without the URL link's accent.
            variant action {
                #[default]
                off(_t) {}
                on(t) {
                    color: t.color.text(),
                    underline: true,
                }
            }
            state hovered(t) { color: t.intent.primary.solid_bg() }
            // Focus mirrors hover (color brighten) — an inline text link takes a
            // text affordance, not a box ring. Replaces the native macOS ring.
            state focused(t) { color: t.intent.primary.solid_bg() }
            transitions { color: 120ms EaseOut }
        }

        /// The pressable around an action link's text: no chrome, a
        /// pointer, and a soft tint while keyboard-focused. The focus is on
        /// this node, not on the text inside it, so the text slot's
        /// `focused` state never fires for an action link; a tint (not a
        /// border) marks it without shifting the text off the line.
        slot action {
            base(t) {
                cursor: Cursor::Pointer,
                border_radius: t.radius.sm(),
                background: runtime_core::Color("transparent".into()),
            }
            state focused(t) { background: t.intent.primary.soft_bg() }
            transitions { background: 120ms EaseOut }
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
    /// Destination URL (`https:`, `mailto:`, `tel:`, …). Ignored when
    /// `on_press` is set.
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub url: Reactive<String>,
    /// Makes the link an in-app action: pressing it (click, tap, Enter /
    /// Space) runs this instead of opening a URL. It renders as a button
    /// to assistive tech, named by `label`. Default `None` (a URL link).
    pub on_press: Option<Rc<dyn Fn()>>,
    /// Optional robot/E2E test id, forwarded to the interactive node (the
    /// `<a>` or the action's pressable). Only honored when idea-ui's
    /// `robot` feature is on; ignored otherwise.
    pub test_id: Option<&'static str>,
}

impl Default for LinkProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            url: Reactive::Static(String::new()),
            on_press: None,
            test_id: None,
            styles: Default::default(),
        }
    }
}

/// Renders a styled link: the label text wrapped in the framework's
/// `external_link` primitive (a real `<a>` on web, the platform URL opener
/// on native), or — with `on_press` — in a `pressable` that runs the action.
#[component]
pub fn Link(props: &LinkProps) -> Element {
    let action = props.on_press.is_some();
    // `label` routes live to the `text()` sink — a `Signal`/`rx!` re-renders
    // the link text in place.
    let text = runtime_core::text(props.label.clone())
        .with_style(
            LinkStyles::text().set(LinkAction::from(action)).overridden(props.styles.text.as_ref()),
        )
        .into_element();

    if let Some(on_press) = props.on_press.clone() {
        let node = runtime_core::pressable(vec![text], move || (on_press)())
            .with_style(LinkStyles::action().overridden(props.styles.action.as_ref()));
        // Accessible name: a static label names the node explicitly. The
        // a11y prop bag is plain data, so a LIVE label sets no explicit
        // name: on web the button's name then follows its text content as
        // it changes, where a snapshot `aria-label` would override it and
        // keep announcing "Show 1 archived" after the count moved on.
        let node = match &props.label {
            Reactive::Static(label) => node.a11y_label(label.clone()),
            Reactive::Dynamic(_) => node,
        };
        #[cfg(feature = "robot")]
        let node = match props.test_id {
            Some(tid) => node.test_id(tid),
            None => node,
        };
        return node.into_element();
    }

    // A live `url` routes to the primitive's reactive `.url()` setter (swaps the
    // `<a href>` in place); a `Static` url just seeds `external_link` once.
    let mut node = runtime_core::external_link(props.url.get(), vec![text]);
    if !props.url.is_static() {
        let url = props.url.clone();
        node = node.url(move || url.get());
    }
    #[cfg(feature = "robot")]
    let node = match props.test_id {
        Some(tid) => node.test_id(tid),
        None => node,
    };
    node.into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::with_test_world;
    use runtime_core::{resolve_style, Cursor};
    use std::cell::Cell;

    fn theme() {
        idea_theme::theme::install_idea_theme(idea_theme::theme::light_theme());
    }

    fn action(label: Reactive<String>, on_press: Rc<dyn Fn()>) -> LinkProps {
        LinkProps { label, on_press: Some(on_press), ..Default::default() }
    }

    /// CrewForge's ask: a link that runs an in-app action ("Show 1
    /// archived") instead of opening a URL. It is a pressable — a button to
    /// assistive tech, keyboard-activated on web — named by its label, and
    /// pressing it runs `on_press`.
    #[test]
    fn on_press_link_is_a_named_pressable_that_runs_the_action() {
        with_test_world(|| {
            theme();
            let pressed = Rc::new(Cell::new(0));
            let count = pressed.clone();
            let props = action(Reactive::Static("Show 1 archived".into()), Rc::new(move || count.set(count.get() + 1)));
            match classify(Link(&props)) {
                P::Pressable { on_click, accessibility, children, .. } => {
                    assert_eq!(accessibility.label.as_deref(), Some("Show 1 archived"));
                    assert_eq!(accessibility.role, None, "the pressable's own (button) role, not a link role");
                    assert!(matches!(classify(children.into_iter().next().unwrap()), P::Text { .. }));
                    on_click();
                    assert_eq!(pressed.get(), 1, "pressing the link runs on_press");
                }
                _ => panic!("an on_press Link renders a pressable"),
            }
        });
    }

    /// A live label sets no explicit accessible name: a snapshot would go
    /// stale as the text changes, and the content names the button.
    #[test]
    fn on_press_link_with_a_live_label_leaves_the_name_to_its_content() {
        with_test_world(|| {
            theme();
            let n = runtime_core::signal(1);
            let props = action(runtime_core::rx!(format!("Show {} archived", n.get())), Rc::new(|| {}));
            match classify(Link(&props)) {
                P::Pressable { accessibility, .. } => assert_eq!(accessibility.label, None),
                _ => panic!("an on_press Link renders a pressable"),
            }
        });
    }

    /// A `url` link is still the `external_link` primitive (not a
    /// pressable), and its text keeps the accent look; the action link's
    /// text is underlined in the body text color.
    #[test]
    fn url_link_keeps_external_link_and_action_text_is_underlined() {
        with_test_world(|| {
            theme();
            let url = LinkProps {
                label: Reactive::Static("Docs".into()),
                url: Reactive::Static("https://example.com".into()),
                ..Default::default()
            };
            assert!(
                !matches!(classify(Link(&url)), P::Pressable { .. }),
                "a url Link stays the external_link primitive"
            );
            let url_text = resolve_style(&LinkStyles::text().into_style_application());
            assert_eq!(url_text.underline, None);
            assert_eq!(url_text.color.as_ref().and_then(|c| c.name()), Some("intent-primary-fg"));

            let props = action(Reactive::Static("Undo".into()), Rc::new(|| {}));
            let P::Pressable { children, .. } = classify(Link(&props)) else { panic!("pressable") };
            let text = match classify(children.into_iter().next().unwrap()) {
                P::Text { style, .. } => style.expect("styled text").resolve(),
                _ => panic!("the label is a text node"),
            };
            assert_eq!(text.underline, Some(true));
            assert_eq!(text.color.as_ref().and_then(|c| c.name()), Some("color-text"));
            assert_eq!(text.cursor, Some(Cursor::Pointer));
        });
    }

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
            theme();
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
