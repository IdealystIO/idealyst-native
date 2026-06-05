//! `Alert` — banner with title + optional body and dismiss button,
//! built on the extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use std::rc::Rc;
//! use idea_ui::extensible::alert::{alert, AlertProps};
//! use idea_theme::extensible::{tone, variant};
//!
//! ui! {
//!     Alert(
//!         title = "Couldn't save",
//!         body = Some("Server returned 503.".to_string()),
//!         tone = tone::Danger,
//!         variant = variant::Soft,
//!         on_dismiss = Some(Rc::new(move || hide_alert())),
//!     )
//! }
//! ```
//!
//! Same Tone + Variant axes as [`badge`](super::badge::badge). Alert
//! has its own padding/font/radius in the base stylesheet, so no
//! Size/Shape axis — adding one would imply a continuous range of
//! banner densities which we don't have a use for yet.

use std::rc::Rc;

use runtime_core::{
    component, resolve_style, ui, Color, Element, IdealystSchema, IntoElement, IntoStyleSource,
    Reactive, StyleApplication, StyleRules, StyleSheet, Tokenized,
};

use idea_theme::extensible::{installed_alert_sheet, tone, variant, ToneRef, VariantRef};

use crate::stylesheets::{AlertBody, AlertTitle, TagClose};

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
    /// When `Some`, a close affordance appears in the top-right.
    pub on_dismiss: Option<Rc<dyn Fn()>>,
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
            on_dismiss: None,
        }
    }
}

/// Renders a banner with a bold title, optional body line, and an
/// optional dismiss button, styled by the tone × variant axes.
#[component]
pub fn Alert(props: &AlertProps) -> Element {
    let title = props.title.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();

    let appearance_key = format!("{}_{}", tone.key(), variant.key());

    // Static style — build-time apply, no flicker (see Button).
    let container_style =
        StyleApplication::new(installed_alert_sheet()).with("appearance", appearance_key);

    // Resolve the fill's foreground so the title, body, and close glyph
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
    let close_style = TagClose();

    let title_node: Element = ui! { text(style = title_style) { title } };
    let body_node: Option<Element> =
        crate::components::optional_reactive_text(props.body.clone(), body_style);

    let close_node: Option<Element> = props.on_dismiss.clone().map(|on_dismiss| {
        // Bare `×` text node — color it directly so it shows on native.
        let close_text = match fg.clone() {
            Some(c) => runtime_core::text("×".to_string())
                .with_style(label_color_only(c))
                .into_element(),
            None => runtime_core::text("×".to_string()).into_element(),
        };
        runtime_core::pressable(vec![close_text], move || (on_dismiss)())
            .with_style(close_style)
            .into_element()
    });

    let mut children: Vec<Element> = Vec::with_capacity(2);
    let mut text_column: Vec<Element> = Vec::with_capacity(2);
    text_column.push(title_node);
    if let Some(b) = body_node {
        text_column.push(b);
    }
    children.push(ui! { view { text_column } });
    if let Some(c) = close_node {
        children.push(c);
    }

    ui! { view(style = container_style) { children } }
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
            on_dismiss: Some(std::rc::Rc::new(|| {})),
        };
        let expected = container_fg();

        let outer = view_children(Alert(&props));
        // [text-column-view, close-pressable]
        let text_column = match &outer[0] {
            Element::View { children, .. } => children,
            _ => panic!("first child is the text column view"),
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
}
