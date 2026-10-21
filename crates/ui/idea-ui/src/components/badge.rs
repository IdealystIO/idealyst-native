//! `Badge` — small pill-shaped status indicator.
//!
//! ```ignore
//! ui! {
//!     Badge(
//!         label = "New",
//!         tone = tone::Success,
//!         variant = variant::Soft,
//!     )
//! }
//! ```
//!
//! Styling is [`BadgeStyles`]: one `root` slot (the pill — a single text
//! node) drawing one compound per `(tone, variant)` over every
//! categorization tone (the semantic tones plus the palette hues, plus any
//! registered app tone). Restyle it or add variants at boot like any
//! component style.

use runtime_core::{
    component, component_styles, text, AlignSelf, Element, FontWeight, IdealystSchema, IntoElement, Reactive,
    SlotRestyle, TextAlign, TextTransform,
};

use idea_theme::extensible::{categorization_tones, tone, ToneRef};

use crate::components::tone_slots::{surface_arms, with_tone};
use crate::modifiers::{marker_into_axis, variant};
use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Badge`].
    pub Badge<IdeaThemeRef> {
        /// The surface treatment — a tone-aware recipe per value.
        axis variant { filled, #[default] soft, outlined, ghost }

        /// The pill.
        slot root {
            extend(root_surfaces)
            base(t) {
                // Hug: keep the pill sized to content instead of stretching
                // to a flex parent's cross axis (which would grow it to the
                // row height and float its label to the top).
                align_self: AlignSelf::Center,
                padding_vertical: 2.0,
                padding_horizontal: t.spacing.sm(),
                border_radius: t.radius.pill(),
                font_size: t.typography.caption_size(),
                font_weight: FontWeight::SemiBold,
                letter_spacing: 0.4,
                text_transform: TextTransform::Uppercase,
                text_align: TextAlign::Center,
            }
        }
    }
}

fn root_surfaces(s: &mut SlotRestyle<'_, BadgeStyles>) {
    surface_arms(s, &categorization_tones());
}

marker_into_axis!(BadgeVariant {
    variant::Filled => Filled,
    variant::Soft => Soft,
    variant::Outlined => Outlined,
    variant::Ghost => Ghost,
});

// Reactive-by-default: `#[props]` wraps `tone`/`variant` → `Reactive<…>`;
// `label` is already reactive. Bare markers (`tone = tone::Success`,
// `variant = variant::Soft`) coerce.
#[runtime_core::props(styles = BadgeStyles)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct BadgeProps {
    /// Badge text. `Reactive<String>` — static or live (signal/`rx!`).
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
    /// Semantic color palette (Neutral, Success, Danger, Warning, …, a
    /// palette hue, or a registered app tone). Default Neutral.
    pub tone: ToneRef,
    /// Surface treatment (Soft, Filled, Outlined, Ghost, or an app value).
    /// Default Soft.
    pub variant: BadgeVariant,
}

impl Default for BadgeProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            tone: tone::Neutral.into(),
            variant: Reactive::Static(BadgeVariant::default()),
            styles: Default::default(),
        }
    }
}

/// Renders a small pill-shaped status indicator: a single styled text
/// node drawn by [`BadgeStyles::root`].
#[component]
pub fn Badge(props: &BadgeProps) -> Element {
    let values = with_tone(BadgeStyles::values().set(props.variant.clone()), &props.tone);
    let style = BadgeStyles::root().values(&values).overridden(props.styles.root.as_ref());
    text(props.label.clone()).with_style(style).into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P, TStyle};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, AlignSelf};

    // Regression: a Badge is an inline pill — it must HUG its content
    // (`align_self: Center`), not inherit a flex parent's default
    // `align-items: stretch` and grow to the row's height, which floated the
    // label to the top of an over-tall pill (the Intents-row report).
    #[test]
    fn badge_hugs_content_not_stretch() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let props = BadgeProps { label: Reactive::Static("New".into()), ..Default::default() };
            let app = match classify(Badge(&props)) {
                P::Text { style, .. } => match style.expect("Badge renders a styled Text node") {
                    TStyle::App(a) => a,
                    _ => panic!("Badge uses a static style source"),
                },
                _ => panic!("Badge renders a styled Text node"),
            };
            assert_eq!(
                resolve_style(&app).align_self,
                Some(AlignSelf::Center),
                "a Badge sizes to content (centered), never stretching to the parent cross axis"
            );
    });
    }
}
