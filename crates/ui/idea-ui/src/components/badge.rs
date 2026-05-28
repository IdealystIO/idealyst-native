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

use runtime_core::{text, IntoElement, Element, Reactive, StyleApplication};

use idea_theme::extensible::{installed_badge_sheet, tone, variant, ToneRef, VariantRef};

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct BadgeProps {
    /// Badge text. `Reactive<String>` — static or live (signal/`rx!`).
    pub label: Reactive<String>,
    pub tone: ToneRef,
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

pub fn badge(props: &BadgeProps) -> Element {
    let label = props.label.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();
    let appearance_key = format!("{}_{}", tone.key(), variant.key());

    // Static style — see Button for why (build-time apply, no flicker).
    let style =
        StyleApplication::new(installed_badge_sheet()).with("appearance", appearance_key);

    text(label).with_style(style).into_element()
}
