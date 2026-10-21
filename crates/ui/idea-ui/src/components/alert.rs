//! `Alert` — banner with title + optional body, an optional trailing
//! action slot, and a configurable close affordance, built on the
//! extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use std::rc::Rc;
//! use idea_ui::{Alert, AlertClose};
//! use idea_ui::{tone, variant};
//!
//! ui! {
//!     Alert(
//!         title = "Couldn't save",
//!         body = Some("Server returned 503.".to_string()),
//!         tone = tone::Danger,
//!         variant = variant::Soft,
//!         // Trailing action slot — any element (a Button here).
//!         action = Some(ui! { Button(label = "Retry", on_click = retry) }),
//!         // Close affordance: `None` (default), `Button(handler)` for the
//!         // standard ×, or `Custom(element)` to supply your own.
//!         close = AlertClose::Button(Rc::new(move || hide_alert())),
//!     )
//! }
//! ```
//!
//! Same tone × variant axes as [`Badge`](super::badge::Badge). Styling is
//! [`AlertStyles`]: `root` (the banner), `content` (the title/body
//! column), `title`, `body`, `close` (the built-in close button) and
//! `glyph` (its `×`). The text slots carry the fill's foreground on their
//! own nodes — native text inherits no color from its container.
//!
//! Layout is a row: a flex-growing title/body column, then the optional
//! `action`, then the optional `close`. The action and close slots take
//! caller-supplied elements verbatim (they carry their own styling and
//! handlers).

use std::rc::Rc;

use runtime_core::{
    component, component_styles, ui, AlignItems, Color, Cursor, Element, FlexDirection, FontWeight,
    IdealystSchema, IntoElement, JustifyContent, Length, Reactive, SlotRestyle, TextAlign,
};

use idea_theme::extensible::{semantic_tones, tone, ToneRef};

use crate::components::tone_slots::{foreground_arms, surface_arms, with_tone};
use crate::modifiers::{marker_into_axis, variant};
use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Alert`].
    pub Alert<IdeaThemeRef> {
        /// The surface treatment — a tone-aware recipe per value.
        axis variant { filled, #[default] soft, outlined, ghost }

        /// The banner.
        slot root {
            extend(root_surfaces)
            base(t) {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                gap: t.spacing.md(),
                padding_vertical: t.spacing.md(),
                padding_horizontal: t.spacing.lg(),
                border_radius: t.radius.md(),
            }
        }

        /// The title/body column. `flex_grow: 1` + `min_width: 0` lets it
        /// take the available width and shrink (wrapping text), so the
        /// trailing `action` and `close` align to the banner's far edge.
        slot content {
            base(_t) {
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                min_width: 0.0,
                // Below the theme's smallest spacing step (`xs` = 4px).
                gap: Length::Px(2.0),
            }
        }

        /// The title, in the fill's foreground.
        slot title {
            extend(text_foregrounds)
            base(t) {
                font_size: t.typography.body_size(),
                font_weight: FontWeight::SemiBold,
                line_height: 20.0,
            }
        }

        /// The body line, in the fill's foreground.
        slot body {
            extend(text_foregrounds)
            base(t) {
                font_size: t.typography.body_sm_size(),
                line_height: 18.0,
            }
        }

        /// The built-in close button: a 16×16 hit box with no fill.
        slot close {
            base(t) {
                background: Color("transparent".into()),
                padding: 0.0,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                cursor: Cursor::Pointer,
                font_size: t.typography.body_size(),
                font_weight: FontWeight::Bold,
                text_align: TextAlign::Center,
                line_height: 14.0,
                width: 16.0,
                height: 16.0,
                border_radius: t.radius.pill(),
            }
            transitions {
                background: 150ms EaseOut,
                opacity: 150ms EaseOut,
            }
        }

        /// The close button's `×`, in the fill's foreground.
        slot glyph {
            extend(text_foregrounds)
            base(_t) {}
        }
    }
}

fn root_surfaces(s: &mut SlotRestyle<'_, AlertStyles>) {
    surface_arms(s, &semantic_tones());
}

fn text_foregrounds(s: &mut SlotRestyle<'_, AlertStyles>) {
    foreground_arms(s, &semantic_tones());
}

marker_into_axis!(AlertVariant {
    variant::Filled => Filled,
    variant::Soft => Soft,
    variant::Outlined => Outlined,
    variant::Ghost => Ghost,
});

/// The close affordance shown at an [`Alert`]'s trailing edge.
///
/// One prop expresses all three modes so there's no "show a close?" flag
/// that has to agree with a separate "what does it do?" handler.
#[derive(runtime_core::Remote)]
pub enum AlertClose {
    /// No close affordance. (Default.)
    None,
    /// The standard `×` glyph; invokes the handler when pressed. Alert
    /// styles and colors it (carrying the intent foreground on native).
    Button(Rc<dyn Fn()>),
    /// A caller-supplied element used in place of the `×`. Taken verbatim
    /// — it carries its own styling and press behaviour.
    Custom(Element),
}

impl Default for AlertClose {
    fn default() -> Self {
        AlertClose::None
    }
}

// Reactive-by-default: `#[props]` wraps the scalar data props (`tone`/`variant`)
// → `Reactive<…>`; `title`/`body` are already `Reactive`. `action` (an
// `Option<Element>`) is skipped (Element isn't wrapped), and `close`
// (`AlertClose`, a custom element-builder enum) is `#[prop(static)]`.
#[runtime_core::props(styles = AlertStyles)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct AlertProps {
    /// Alert title. `Reactive<String>` — static or live (signal/`rx!`).
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub title: Reactive<String>,
    /// Optional second-line detail text, beneath the title.
    /// `Reactive<Option<String>>` — static or live.
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub body: Reactive<Option<String>>,
    /// Semantic color palette (Info, Danger, Warning, Success, …).
    /// Default Info.
    pub tone: ToneRef,
    /// Surface treatment (Soft, Filled, Outlined, Ghost, or an app value).
    /// Default Soft.
    pub variant: AlertVariant,
    /// Optional trailing action slot — e.g. an "Undo"/"Retry" `Button`,
    /// or any element. Rendered after the text column, before `close`.
    /// Taken verbatim (carries its own styling and handlers).
    pub action: Option<Element>,
    /// Close affordance at the trailing edge. See [`AlertClose`]. Default
    /// [`AlertClose::None`] (no close).
    #[prop(static)]
    pub close: AlertClose,
}

impl Default for AlertProps {
    fn default() -> Self {
        Self {
            title: Reactive::Static(String::new()),
            body: Reactive::Static(None),
            // Info/Soft = the common informational alert. Use Danger/Filled
            // for breaking news, Warning/Soft for cautionary, etc.
            tone: tone::Info.into(),
            variant: Reactive::Static(AlertVariant::default()),
            action: None,
            close: AlertClose::None,
            styles: Default::default(),
        }
    }
}

/// Renders a banner with a bold title, optional body line, an optional
/// trailing action slot, and an optional close affordance, styled by the
/// tone × variant axes.
#[component]
pub fn Alert(props: AlertProps) -> Element {
    let styles = props.styles.clone();
    let values = with_tone(AlertStyles::values().set(props.variant.clone()), &props.tone);

    let title = props.title.clone();
    let title_style = AlertStyles::title().values(&values).overridden(styles.title.as_ref());
    let title_node: Element = ui! { text(style = title_style) { title } };
    let body_node: Option<Element> = crate::components::optional_reactive_text(
        props.body.clone(),
        AlertStyles::body().values(&values).overridden(styles.body.as_ref()),
    );

    // Trailing slots. The action element is used verbatim; the close
    // affordance is built from `AlertClose`.
    let action_node: Option<Element> = props.action;
    let close_node: Option<Element> = match props.close {
        AlertClose::None => None,
        AlertClose::Button(on_press) => {
            let glyph = runtime_core::text("×".to_string())
                .with_style(AlertStyles::glyph().values(&values).overridden(styles.glyph.as_ref()))
                .into_element();
            Some(
                runtime_core::pressable(vec![glyph], move || (on_press)())
                    .with_style(AlertStyles::close().values(&values).overridden(styles.close.as_ref()))
                    .into_element(),
            )
        }
        AlertClose::Custom(el) => Some(el),
    };

    let content_style = AlertStyles::content().values(&values).overridden(styles.content.as_ref());
    let content_col: Element = ui! {
        view(style = content_style) {
            title_node
            body_node
        }
    };

    // Outer row: content column, then optional action, then optional close.
    let mut children: Vec<Element> = Vec::with_capacity(3);
    children.push(content_col);
    if let Some(action) = action_node {
        children.push(action);
    }
    if let Some(close) = close_node {
        children.push(close);
    }
    runtime_core::view(children)
        .with_style(AlertStyles::root().values(&values).overridden(styles.root.as_ref()))
        .into_element()
}

#[cfg(test)]
mod tests {

    /// The text-slot sheets exist to take Alert's title/body/glyph OFF
    /// the live style engine under `--premint`: the former call-site
    /// composition (resolve container → copy color onto an anonymous
    /// sheet) had no premint class by construction. Every text
    /// application must therefore carry a preminted class list for
    /// every appearance the container itself premints.
    #[test]
    fn regression_alert_text_slots_premint() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            for slot in [AlertStyles::title(), AlertStyles::body(), AlertStyles::glyph()] {
                let app = slot.raw("tone", "danger").set(AlertVariant::Filled).into_style_application();
                assert!(
                    app.preminted_class_list().is_some(),
                    "alert text sheet must premint (was call-site composition)"
                );
            }
        });
    }

    use super::*;
    use crate::test_support::{classify, P, TStyle};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::resolve_style;

    fn theme() {
        install_idea_theme(light_theme());
    }

    fn view_children(el: Element) -> Vec<Element> {
        match classify(el) {
            P::View { children, .. } => children,
            _ => panic!("Alert renders a View"),
        }
    }

    fn text_node_color(el: Element) -> Option<runtime_core::Color> {
        match classify(el) {
            P::Text { style, .. } => {
                let app = match style? {
                    TStyle::App(a) => a,
                    _ => panic!("Alert text uses a static style"),
                };
                resolve_style(&app).color.clone().map(|c| c.resolve())
            }
            _ => None,
        }
    }

    fn container_fg() -> runtime_core::Color {
        let app = AlertStyles::root().raw("tone", "primary").set(AlertVariant::Filled).into_style_application();
        resolve_style(&app)
            .color
            .clone()
            .expect("the filled container resolves a foreground")
            .resolve()
    }

    // Field report 3.1b: the bare title/body text nodes were colored only
    // via the container appearance, so they vanished on native (no
    // parent-color inheritance). Each text node must carry the intent
    // foreground itself. Assert the title, body, AND close glyph carry the
    // filled container's resolved color (white intent-primary-solid-text).
    #[test]
    fn regression_filled_alert_text_nodes_carry_intent_text_color() {
        with_test_world(|| {
            theme();
            let props = AlertProps {
                title: Reactive::Static("Saved".into()),
                body: Reactive::Static(Some("All changes persisted.".into())),
                tone: tone::Primary.into(),
                variant: variant::Filled.into(),
                close: AlertClose::Button(Rc::new(|| {})),
                ..Default::default()
            };
            let expected = container_fg();

            let mut outer = view_children(Alert(props));
            // [content-view, close-pressable]
            let close = outer.remove(1);
            let mut text_column = match classify(outer.remove(0)) {
                P::View { children, .. } => children,
                _ => panic!("first child is the content view"),
            };
            // title + body
            let body = text_column.remove(1);
            let title_color =
                text_node_color(text_column.remove(0)).expect("title carries its own color");
            assert_eq!(title_color, expected, "title is the intent text color");
            let body_color = text_node_color(body).expect("body carries its own color");
            assert_eq!(body_color, expected, "body is the intent text color");

            // close `×`
            let close_glyph = match classify(close) {
                P::Pressable { mut children, .. } => children.remove(0),
                _ => panic!("close is a Pressable"),
            };
            let close_color = text_node_color(close_glyph).expect("close glyph carries its own color");
            assert_eq!(close_color, expected, "close glyph is the intent text color");

            assert_eq!(expected.0.to_ascii_lowercase(), "#ffffff");
    });
    }

    /// The trailing slots render in order: content column, then the
    /// `action` element (verbatim), then the close affordance.
    #[test]
    fn renders_action_and_close_slots_in_order() {
        with_test_world(|| {
            theme();
            let action = runtime_core::text("Retry".to_string()).into_element();
            let props = AlertProps {
                title: Reactive::Static("Couldn't save".into()),
                tone: tone::Danger.into(),
                variant: variant::Soft.into(),
                action: Some(action),
                close: AlertClose::Button(Rc::new(|| {})),
                ..Default::default()
            };

            let outer = view_children(Alert(props));
            assert_eq!(outer.len(), 3, "content + action + close");
            let kinds: Vec<P> = outer.into_iter().map(classify).collect();
            // The action slot is the bare text node we passed, used verbatim.
            assert!(
                matches!(kinds[1], P::Text { .. }),
                "action slot renders the provided element"
            );
            assert!(matches!(kinds[2], P::Pressable { .. }), "close is a Pressable");
    });
    }

    /// `AlertClose::Custom` uses the supplied element verbatim instead of
    /// building the standard `×` Pressable.
    #[test]
    fn close_custom_renders_provided_element() {
        with_test_world(|| {
            theme();
            let custom = runtime_core::text("done".to_string()).into_element();
            let props = AlertProps {
                title: Reactive::Static("hi".into()),
                close: AlertClose::Custom(custom),
                ..Default::default()
            };
            let mut outer = view_children(Alert(props));
            // [content-view, custom-close-text] — no Pressable wrapper.
            assert_eq!(outer.len(), 2);
            assert!(
                matches!(classify(outer.remove(1)), P::Text { .. }),
                "custom close element is used verbatim"
            );
    });
    }

    /// `AlertClose::None` (the default) emits no close affordance.
    #[test]
    fn close_none_omits_affordance() {
        with_test_world(|| {
            theme();
            let props = AlertProps {
                title: Reactive::Static("hi".into()),
                ..Default::default()
            };
            let outer = view_children(Alert(props));
            assert_eq!(outer.len(), 1, "no action, no close → just the content column");
    });
    }
}
