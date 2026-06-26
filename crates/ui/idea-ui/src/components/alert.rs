//! `Alert` — banner with title + optional body, an optional trailing
//! action slot, and a configurable close affordance, built on the
//! extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use std::rc::Rc;
//! use idea_ui::{Alert, AlertClose};
//! use idea_theme::extensible::{tone, variant};
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
//! Same Tone + Variant axes as [`badge`](super::badge::badge). Alert
//! has its own padding/font/radius in the base stylesheet, so no
//! Size/Shape axis — adding one would imply a continuous range of
//! banner densities which we don't have a use for yet.
//!
//! Layout is a row: a flex-growing title/body column, then the optional
//! `action`, then the optional `close`. The action and close slots take
//! caller-supplied elements verbatim (they carry their own styling and
//! handlers); only the built-in `×` close and the title/body get Alert's
//! native text-color stamping.

use std::rc::Rc;

use runtime_core::{
    component, resolve_style, ui, Color, Element, IdealystSchema, IntoElement, IntoStyleSource,
    Reactive, StyleApplication, StyleRules, StyleSheet, Tokenized,
};

use idea_theme::extensible::{installed_alert_sheet, tone, variant, ToneRef, VariantRef};

use crate::stylesheets::{AlertBody, AlertContent, AlertTitle, TagClose};

/// Resolves `text_style` and overlays the parent fill's foreground
/// `color` onto its own node. Native `UILabel`/`TextView` don't inherit
/// text color from a parent (only web's CSS cascade does), so a label
/// colored solely via its wrapping container vanishes on the fill on
/// iOS/Android. Stamping the resolved `color` on the text node makes
/// every backend match web — the pattern `Typography` uses. Tokens are
/// preserved, so theme swaps still re-resolve in bulk.
fn with_inherited_color(text_style: impl IntoStyleSource, color: Tokenized<Color>) -> Rc<StyleSheet> {
    let app = match text_style.into_style_source() {
        runtime_core::StyleSource::Static(a) => a,
        _ => unreachable!("label style sheets are static"),
    };
    let mut rules = (*resolve_style(&app)).clone();
    rules.color = Some(color);
    Rc::new(StyleSheet::r#static(rules))
}

/// A color-only static sheet for a bare leaf text node (the `×` glyph).
fn label_color_only(color: Tokenized<Color>) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        color: Some(color),
        ..Default::default()
    }))
}

/// The close affordance shown at an [`Alert`]'s trailing edge.
///
/// One prop expresses all three modes so there's no "show a close?" flag
/// that has to agree with a separate "what does it do?" handler.
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
#[runtime_core::props]
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
    /// Surface treatment (Soft, Filled, Outline, …). Default Soft.
    pub variant: VariantRef,
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
            variant: variant::Soft.into(),
            action: None,
            close: AlertClose::None,
            // (tone/variant: marker `.into()` → `Reactive<…>`; title/body
            // already `Reactive::Static`; action/close unwrapped.)
        }
    }
}

/// Renders a banner with a bold title, optional body line, an optional
/// trailing action slot, and an optional close affordance, styled by the
/// tone × variant axes.
#[component]
pub fn Alert(props: AlertProps) -> Element {
    // TODO(reactive-sweep): route `tone`/`variant` reactively to the
    // appearance/foreground sinks. They drive a COUPLED, eager structure here —
    // the container appearance key, the resolved foreground color, and the
    // per-node text-color stamping on title/body/`×` (native doesn't inherit
    // color) all derive from tone × variant at build time. A live signal would
    // need every one of those re-resolved in a style closure (the badge.rs
    // make_style/style_is_reactive gate), which is a structural rewrite of the
    // color-stamping path. Snapshotting here keeps tone/variant static-correct.
    let appearance_key = format!("{}_{}", props.tone.get().key(), props.variant.get().key());

    // Static style — build-time apply, no flicker (see Button).
    let container_style =
        StyleApplication::new(installed_alert_sheet()).with("appearance", appearance_key);

    // Resolve the fill's foreground so the title, body, and `×` glyph
    // carry it on their own text nodes (native doesn't inherit color).
    let fg = resolve_style(&container_style).color.clone();

    let title_style: Rc<StyleSheet> = match fg.clone() {
        Some(c) => with_inherited_color(AlertTitle(), c),
        None => AlertTitle::sheet(),
    };
    let body_style: Rc<StyleSheet> = match fg.clone() {
        Some(c) => with_inherited_color(AlertBody(), c),
        None => AlertBody::sheet(),
    };

    let title = props.title.clone();
    let body_node: Option<Element> =
        crate::components::optional_reactive_text(props.body.clone(), body_style);

    // Trailing slots. The action element is used verbatim; the close
    // affordance is built from `AlertClose`.
    let action_node: Option<Element> = props.action;
    let close_node: Option<Element> = match props.close {
        AlertClose::None => None,
        AlertClose::Button(on_press) => {
            // Bare `×` text node — color it directly so it shows on native.
            let close_text = match fg.clone() {
                Some(c) => runtime_core::text("×".to_string())
                    .with_style(label_color_only(c))
                    .into_element(),
                None => runtime_core::text("×".to_string()).into_element(),
            };
            Some(
                runtime_core::pressable(vec![close_text], move || (on_press)())
                    .with_style(TagClose())
                    .into_element(),
            )
        }
        AlertClose::Custom(el) => Some(el),
    };

    let content_style = AlertContent();
    ui! {
        view(style = container_style) {
            view(style = content_style) {
                text(style = title_style) { title }
                body_node
            }
            action_node
            close_node
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, StyleSource};

    fn theme() {
        install_idea_theme(light_theme());
    }

    fn view_children(el: Element) -> Vec<Element> {
        match el {
            Element::View { children, .. } => children,
            _ => panic!("Alert renders a View"),
        }
    }

    fn text_node_color(el: &Element) -> Option<runtime_core::Color> {
        match el {
            Element::Text { style, .. } => {
                let app = match style.as_ref()? {
                    StyleSource::Static(a) => a.clone(),
                    _ => panic!("Alert text uses a static style"),
                };
                resolve_style(&app).color.clone().map(|c| c.resolve())
            }
            _ => None,
        }
    }

    fn container_fg() -> runtime_core::Color {
        let app = StyleApplication::new(installed_alert_sheet())
            .with("appearance", "primary_filled".to_string());
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
    // filled container's resolved color (white intent-primary-solid-text) —
    // an assertion the old uncolored nodes would have failed.
    #[test]
    fn regression_filled_alert_text_nodes_carry_intent_text_color() {
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

        let outer = view_children(Alert(props));
        // [content-view, close-pressable]
        let text_column = match &outer[0] {
            Element::View { children, .. } => children,
            _ => panic!("first child is the content view"),
        };
        // title + body
        let title_color = text_node_color(&text_column[0]).expect("title carries its own color");
        assert_eq!(title_color, expected, "title is the intent text color");
        let body_color = text_node_color(&text_column[1]).expect("body carries its own color");
        assert_eq!(body_color, expected, "body is the intent text color");

        // close `×`
        let close_glyph = match &outer[1] {
            Element::Pressable { children, .. } => &children[0],
            _ => panic!("close is a Pressable"),
        };
        let close_color = text_node_color(close_glyph).expect("close glyph carries its own color");
        assert_eq!(close_color, expected, "close glyph is the intent text color");

        assert_eq!(expected.0.to_ascii_lowercase(), "#ffffff");
    }

    /// The trailing slots render in order: content column, then the
    /// `action` element (verbatim), then the close affordance.
    #[test]
    fn renders_action_and_close_slots_in_order() {
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
        // The action slot is the bare text node we passed, used verbatim.
        match &outer[1] {
            Element::Text { .. } => {}
            _ => panic!("action slot renders the provided element"),
        }
        match &outer[2] {
            Element::Pressable { .. } => {}
            _ => panic!("close is a Pressable"),
        }
    }

    /// `AlertClose::Custom` uses the supplied element verbatim instead of
    /// building the standard `×` Pressable.
    #[test]
    fn close_custom_renders_provided_element() {
        theme();
        let custom = runtime_core::text("done".to_string()).into_element();
        let props = AlertProps {
            title: Reactive::Static("hi".into()),
            close: AlertClose::Custom(custom),
            ..Default::default()
        };
        let outer = view_children(Alert(props));
        // [content-view, custom-close-text] — no Pressable wrapper.
        assert_eq!(outer.len(), 2);
        match &outer[1] {
            Element::Text { .. } => {}
            _ => panic!("custom close element is used verbatim"),
        }
    }

    /// `AlertClose::None` (the default) emits no close affordance.
    #[test]
    fn close_none_omits_affordance() {
        theme();
        let props = AlertProps {
            title: Reactive::Static("hi".into()),
            ..Default::default()
        };
        let outer = view_children(Alert(props));
        assert_eq!(outer.len(), 1, "no action, no close → just the content column");
    }
}
