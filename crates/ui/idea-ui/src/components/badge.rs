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
//! Styling comes from the installed Badge stylesheet. Apps with
//! custom tones/variants install an extended sheet via
//! `install_badge_sheet(BadgeSheetBuilder::new().add_tone(...).build())`
//! before mounting. The default sheet is installed by
//! `install_idea_theme`.

use runtime_core::{component, text, IdealystSchema, IntoElement, Element, Reactive, StyleApplication};

use idea_theme::extensible::{installed_badge_sheet, tone, variant, ToneRef, VariantRef};

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct BadgeProps {
    /// Badge text. `Reactive<String>` — static or live (signal/`rx!`).
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
    /// Semantic color palette (Neutral, Success, Danger, Warning, …).
    /// Default Neutral.
    pub tone: ToneRef,
    /// Surface treatment (Soft, Filled, Outline, …). Default Soft.
    pub variant: VariantRef,
}

impl Default for BadgeProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(String::new()),
            tone: tone::Neutral.into(),
            variant: variant::Soft.into(),
        }
    }
}

/// Renders a small pill-shaped status indicator: a single styled text
/// node whose tone × variant appearance is resolved from the installed
/// Badge stylesheet.
#[component]
pub fn Badge(props: &BadgeProps) -> Element {
    let label = props.label.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();
    let appearance_key = format!("{}_{}", tone.key(), variant.key());

    // Static style — see Button for why (build-time apply, no flicker).
    let style = StyleApplication::new(installed_badge_sheet())
        .with("appearance", appearance_key)
        .with_computed("hug", crate::components::hug_self);

    text(label).with_style(style).into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, AlignSelf, StyleSource};

    // Regression: a Badge is an inline pill — it must HUG its content
    // (`align_self: Center`), not inherit a flex parent's default
    // `align-items: stretch` and grow to the row's height, which floated the
    // label to the top of an over-tall pill (the Intents-row report).
    #[test]
    fn badge_hugs_content_not_stretch() {
        install_idea_theme(light_theme());
        let props = BadgeProps { label: Reactive::Static("New".into()), ..Default::default() };
        let app = match Badge(&props) {
            Element::Text { style: Some(StyleSource::Static(a)), .. } => a,
            _ => panic!("Badge renders a styled Text node"),
        };
        assert_eq!(
            resolve_style(&app).align_self,
            Some(AlignSelf::Center),
            "a Badge sizes to content (centered), never stretching to the parent cross axis"
        );
    }
}
