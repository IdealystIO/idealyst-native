//! `Tag` — labelled pill with optional close button, built on the
//! extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use std::rc::Rc;
//! use idea_ui::extensible::tag::{tag, TagProps};
//! use idea_theme::extensible::{tone, variant};
//!
//! ui! {
//!     Tag(
//!         label = "Rust",
//!         tone = tone::Primary,
//!         variant = variant::Soft,
//!         on_remove = Some(Rc::new(move || remove("Rust"))),
//!     )
//! }
//! ```
//!
//! Same Tone + Variant axes as [`badge`](super::badge::badge) — the
//! only difference is the optional close affordance. Reuses
//! [`Tag`](crate::stylesheets::Tag) base sheet for the container
//! and [`TagLabel`](crate::stylesheets::TagLabel)/[`TagClose`](crate::stylesheets::TagClose)
//! for the children.

use std::rc::Rc;

use runtime_core::{
    component, resolve_style, ui, Color, Element, IdealystSchema, IntoElement, IntoStyleSource,
    Reactive, StyleApplication, StyleRules, StyleSheet, Tokenized,
};

use idea_theme::extensible::{installed_tag_sheet, tone, variant, ToneRef, VariantRef};

use crate::stylesheets::{TagClose, TagLabel};

/// Resolves `text_style` and overlays the parent fill's foreground
/// `color` onto its own node.
///
/// Native `UILabel`/`TextView` do NOT inherit text color from a parent
/// (only web's CSS cascade does), so a label colored solely via its
/// wrapping container renders invisible on the colored fill on
/// iOS/Android. Resolving the container's `color` and stamping it on the
/// label node makes every backend match web — the same pattern
/// `Typography` uses (color lives on the text node). The merged
/// `Tokenized` values keep their token references, so theme swaps still
/// re-resolve in bulk via the cohort.
fn with_inherited_color(text_style: impl IntoStyleSource, color: Tokenized<Color>) -> Rc<StyleSheet> {
    let app = match text_style.into_style_source() {
        runtime_core::StyleSource::Static(a) => a,
        // The label sheets are constant builders → always Static.
        _ => unreachable!("label style sheets are static"),
    };
    let mut rules = (*resolve_style(&app)).clone();
    rules.color = Some(color);
    Rc::new(StyleSheet::r#static(rules))
}

/// A color-only static sheet for a bare leaf text node (the `×` glyph),
/// which carries no sizing sheet of its own. Same native-inheritance
/// rationale as [`with_inherited_color`].
fn label_color_only(color: Tokenized<Color>) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        color: Some(color),
        ..Default::default()
    }))
}

// Reactive-by-default: `#[props]` wraps `tone`/`variant` → `Reactive<…>`;
// `label` is already reactive and `on_remove` (an `Rc<dyn Fn()>` handler) is
// auto-skipped. Bare markers (`tone = tone::Primary`) coerce to
// `Reactive<ToneRef>` via the marker's generated `From`. The style-driving
// props route into the container-style sink, read `.get()` INSIDE so the
// apply-style Effect subscribes to whichever are live.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct TagProps {
    /// Tag text. `Reactive<String>` — static or live (signal/`rx!`).
    pub label: Reactive<String>,
    /// Semantic color palette. Default Neutral.
    pub tone: ToneRef,
    /// Surface treatment (Soft, Filled, Outline, …). Default Soft.
    pub variant: VariantRef,
    /// When `Some`, a close button renders to the right of the label.
    pub on_remove: Option<Rc<dyn Fn()>>,
}

impl Default for TagProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            tone: tone::Neutral.into(),
            variant: variant::Soft.into(),
            on_remove: None,
        }
    }
}

/// Renders a tone/variant-styled pill containing `label`, with an
/// optional close button (when `on_remove` is set) to its right.
#[component]
pub fn Tag(props: &TagProps) -> Element {
    let label = props.label.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();

    // The container style is REACTIVE when tone/variant is live; else the
    // build-time fast path (no flicker — see Button). The closure reads each
    // prop's `.get()` INSIDE so the apply-style Effect subscribes to whichever
    // are dynamic. The `hug` layer keeps the tag sized to content instead of
    // stretching to a flex parent's row height (see `components::hug_self`).
    let style_is_reactive = !tone.is_static() || !variant.is_static();
    let make_container_style = {
        let tone = tone.clone();
        let variant = variant.clone();
        move || {
            let appearance_key = format!("{}_{}", tone.get().key(), variant.get().key());
            StyleApplication::new(installed_tag_sheet())
                .with("appearance", appearance_key)
                .with_computed("hug", crate::components::hug_self)
        }
    };

    // Build-time container style: also used to resolve the fill's foreground
    // so the label + close glyph carry it on their OWN text nodes (native
    // doesn't inherit text color). Reads the tone/variant snapshot.
    //
    // TODO(reactive-sweep): the label/close foreground COLOR is a coupled
    // sink — it's resolved from the container fill at build time and stamped
    // onto the separate text nodes. When tone/variant are reactive the
    // container border/fill re-styles in place (the closure above), but the
    // label/close text color is snapshotted here and won't track a live
    // tone/variant change. Routing it needs the label color resolved INSIDE a
    // reactive style closure on the label node (read tone/variant `.get()`
    // there too) — same shape as Typography's color sink. Left as a follow-on
    // because it's a derived-from-resolved-style coupling, not a plain prop.
    let container_style = make_container_style();
    let fg = resolve_style(&container_style).color.clone();

    let label_style: Rc<StyleSheet> = match fg.clone() {
        Some(c) => with_inherited_color(TagLabel(), c),
        None => TagLabel::sheet(),
    };
    let close_style = TagClose();

    let label_el: Element = ui! { text(style = label_style) { label } };

    let mut children: Vec<Element> = Vec::with_capacity(2);
    children.push(label_el);
    if let Some(on_remove) = props.on_remove.clone() {
        // The `×` is a bare text node inside the pressable; color it on
        // its own node so it's visible on native (TagClose only sizes
        // the affordance and "inherits" foreground — which native won't).
        let close_text = match fg.clone() {
            Some(c) => runtime_core::text("×".to_string())
                .with_style(label_color_only(c))
                .into_element(),
            None => runtime_core::text("×".to_string()).into_element(),
        };
        let close = runtime_core::pressable(vec![close_text], move || (on_remove)())
            .with_style(close_style)
            .into_element();
        children.push(close);
    }

    let node = runtime_core::view(children);
    if style_is_reactive {
        node.with_style(make_container_style).into_element()
    } else {
        node.with_style(container_style).into_element()
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
            _ => panic!("Tag renders a View"),
        }
    }

    fn text_node_color(el: &Element) -> Option<runtime_core::Color> {
        match el {
            Element::Text { style, .. } => {
                let app = match style.as_ref()? {
                    StyleSource::Static(a) => a.clone(),
                    _ => panic!("Tag label uses a static style"),
                };
                resolve_style(&app).color.clone().map(|c| c.resolve())
            }
            _ => None,
        }
    }

    /// The intent foreground the filled container resolves to — the color
    /// the label MUST carry on its own node (native won't inherit it).
    fn container_fg() -> runtime_core::Color {
        let app = StyleApplication::new(installed_tag_sheet())
            .with("appearance", "primary_filled".to_string());
        resolve_style(&app)
            .color
            .clone()
            .expect("the filled container resolves a foreground")
            .resolve()
    }

    // Field report 3.1b: a bare label colored only via the container's
    // appearance vanished on native (no parent-color inheritance). The
    // label node must carry the intent foreground itself. A test that
    // passed against the old bare/uncolored label is not a valid
    // regression — so we assert the label node's OWN resolved color equals
    // the filled container's foreground (white intent-primary-solid-text).
    #[test]
    fn regression_filled_tag_label_carries_intent_text_color() {
        theme();
        let props = TagProps {
            label: Reactive::Static("Rust".into()),
            tone: tone::Primary.into(),
            variant: variant::Filled.into(),
            ..Default::default()
        };
        let children = view_children(Tag(&props));
        let color = text_node_color(&children[0])
            .expect("tag label must carry its own color, not inherit from the container");
        assert_eq!(color, container_fg());
        assert_eq!(color.0.to_ascii_lowercase(), "#ffffff");
    }

    // The close `×` is also a bare text node; it must carry the color too.
    #[test]
    fn regression_filled_tag_close_glyph_carries_intent_text_color() {
        theme();
        let props = TagProps {
            label: Reactive::Static("Rust".into()),
            tone: tone::Primary.into(),
            variant: variant::Filled.into(),
            on_remove: Some(std::rc::Rc::new(|| {})),
        };
        let children = view_children(Tag(&props));
        // [label, close-pressable]; the close glyph is the pressable's child.
        let close_glyph = match &children[1] {
            Element::Pressable { children, .. } => &children[0],
            _ => panic!("close is a Pressable"),
        };
        let color = text_node_color(close_glyph)
            .expect("close glyph must carry its own color");
        assert_eq!(color, container_fg());
    }
}
