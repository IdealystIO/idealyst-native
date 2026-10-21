//! `Tag` — labelled pill with optional close button, built on the
//! extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use std::rc::Rc;
//! use idea_ui::{tone, variant, Tag};
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
//! Same tone × variant axes as [`Badge`](super::badge::Badge) — the only
//! difference is the optional close affordance. Styling is [`TagStyles`]:
//! `root` (the pill), `label`, `close` (the close button) and `glyph` (its
//! `×`). The label and the `×` carry the fill's foreground on their own
//! nodes: native text inherits no color from its container.

use std::rc::Rc;

use runtime_core::{
    component, component_styles, ui, AlignItems, AlignSelf, Color, Cursor, Element, FlexDirection, FontWeight,
    IdealystSchema, IntoElement, JustifyContent, Reactive, SlotRestyle, TextAlign,
};

use idea_theme::extensible::{categorization_tones, tone, ToneRef};

use crate::components::tone_slots::{foreground_arms, surface_arms, with_tone};
use crate::modifiers::{marker_into_axis, variant};
use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Tag`].
    pub Tag<IdeaThemeRef> {
        /// The surface treatment — a tone-aware recipe per value.
        axis variant { filled, #[default] soft, outlined, ghost }

        /// The pill.
        slot root {
            extend(root_surfaces)
            base(t) {
                // Hug: sized to content, not stretched to a flex parent's
                // cross axis.
                align_self: AlignSelf::Center,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                gap: t.spacing.xs(),
                padding_vertical: 2.0,
                padding_horizontal: t.spacing.sm(),
                border_radius: t.radius.pill(),
                font_size: t.typography.caption_size(),
                font_weight: FontWeight::SemiBold,
                letter_spacing: 0.4,
                text_align: TextAlign::Center,
            }
        }

        /// The label text, in the fill's foreground.
        slot label {
            extend(text_foregrounds)
            base(t) {
                font_size: t.typography.body_sm_size(),
                font_weight: FontWeight::SemiBold,
                letter_spacing: 0.3,
            }
        }

        /// The close button: a 16×16 hit box with no fill of its own.
        slot close {
            base(t) {
                background: Color("transparent".into()),
                padding: 0.0,
                // The `×` is a child text node, so flex-center it within the
                // box — `text_align` alone only centers glyphs inside a text
                // node, not the node within this container.
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

fn root_surfaces(s: &mut SlotRestyle<'_, TagStyles>) {
    surface_arms(s, &categorization_tones());
}

fn text_foregrounds(s: &mut SlotRestyle<'_, TagStyles>) {
    foreground_arms(s, &categorization_tones());
}

marker_into_axis!(TagVariant {
    variant::Filled => Filled,
    variant::Soft => Soft,
    variant::Outlined => Outlined,
    variant::Ghost => Ghost,
});

// Reactive-by-default: `#[props]` wraps `tone`/`variant` → `Reactive<…>`;
// `label` is already reactive and `on_remove` (an `Rc<dyn Fn()>` handler) is
// auto-skipped. Bare markers (`tone = tone::Primary`, `variant =
// variant::Soft`) coerce.
#[runtime_core::props(styles = TagStyles)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct TagProps {
    /// Tag text. `Reactive<String>` — static or live (signal/`rx!`).
    pub label: Reactive<String>,
    /// Semantic color palette. Default Neutral.
    pub tone: ToneRef,
    /// Surface treatment (Soft, Filled, Outlined, Ghost, or an app value).
    /// Default Soft.
    pub variant: TagVariant,
    /// When `Some`, a close button renders to the right of the label.
    pub on_remove: Option<Rc<dyn Fn()>>,
}

impl Default for TagProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            tone: tone::Neutral.into(),
            variant: Reactive::Static(TagVariant::default()),
            on_remove: None,
            styles: Default::default(),
        }
    }
}

/// Renders a tone/variant-styled pill containing `label`, with an
/// optional close button (when `on_remove` is set) to its right.
#[component]
pub fn Tag(props: &TagProps) -> Element {
    let label = props.label.clone();
    let styles = props.styles.clone();
    let values = with_tone(TagStyles::values().set(props.variant.clone()), &props.tone);

    let label_style = TagStyles::label().values(&values).overridden(styles.label.as_ref());
    let label_el: Element = ui! { text(style = label_style) { label } };

    let mut children: Vec<Element> = Vec::with_capacity(2);
    children.push(label_el);
    if let Some(on_remove) = props.on_remove.clone() {
        let glyph = runtime_core::text("×".to_string())
            .with_style(TagStyles::glyph().values(&values).overridden(styles.glyph.as_ref()))
            .into_element();
        let close = runtime_core::pressable(vec![glyph], move || (on_remove)())
            .with_style(TagStyles::close().values(&values).overridden(styles.close.as_ref()))
            .into_element();
        children.push(close);
    }

    runtime_core::view(children)
        .with_style(TagStyles::root().values(&values).overridden(styles.root.as_ref()))
        .into_element()
}

#[cfg(test)]
mod tests {

    /// Mirror of the Alert regression: the label/glyph slots must
    /// premint — the point of replacing the call-site composition.
    #[test]
    fn regression_tag_text_slots_premint() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            for slot in [TagStyles::label(), TagStyles::glyph()] {
                let app = slot.raw("tone", "success").set(TagVariant::Soft).into_style_application();
                assert!(
                    app.preminted_class_list().is_some(),
                    "tag text sheet must premint (was call-site composition)"
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
            _ => panic!("Tag renders a View"),
        }
    }

    fn text_node_color(el: Element) -> Option<runtime_core::Color> {
        match classify(el) {
            P::Text { style, .. } => {
                let app = match style? {
                    TStyle::App(a) => a,
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
        let app = TagStyles::root().raw("tone", "primary").set(TagVariant::Filled).into_style_application();
        resolve_style(&app)
            .color
            .clone()
            .expect("the filled container resolves a foreground")
            .resolve()
    }

    // Field report 3.1b: a bare label colored only via the container's
    // appearance vanished on native (no parent-color inheritance). The
    // label node must carry the intent foreground itself, so we assert
    // the label node's OWN resolved color equals
    // the filled container's foreground (white intent-primary-solid-text).
    #[test]
    fn regression_filled_tag_label_carries_intent_text_color() {
        with_test_world(|| {
            theme();
            let props = TagProps {
                label: Reactive::Static("Rust".into()),
                tone: tone::Primary.into(),
                variant: variant::Filled.into(),
                ..Default::default()
            };
            let mut children = view_children(Tag(&props));
            let color = text_node_color(children.remove(0))
                .expect("tag label must carry its own color, not inherit from the container");
            assert_eq!(color, container_fg());
            assert_eq!(color.0.to_ascii_lowercase(), "#ffffff");
    });
    }

    // The close `×` is also a bare text node; it must carry the color too.
    #[test]
    fn regression_filled_tag_close_glyph_carries_intent_text_color() {
        with_test_world(|| {
            theme();
            let props = TagProps {
                label: Reactive::Static("Rust".into()),
                tone: tone::Primary.into(),
                variant: variant::Filled.into(),
                on_remove: Some(std::rc::Rc::new(|| {})),
                ..Default::default()
            };
            let mut children = view_children(Tag(&props));
            // [label, close-pressable]; the close glyph is the pressable's child.
            let close_glyph = match classify(children.remove(1)) {
                P::Pressable { mut children, .. } => children.remove(0),
                _ => panic!("close is a Pressable"),
            };
            let color = text_node_color(close_glyph)
                .expect("close glyph must carry its own color");
            assert_eq!(color, container_fg());
    });
    }
}
