//! `Badge` — small pill-shaped status indicator. Same intent +
//! kind vocabulary as [`Button`](super::button::Button); kinds are
//! limited to Solid / Soft / Outlined (Ghost doesn't apply — a badge
//! needs a visible surface to read as a chip).
//!
//! ```ignore
//! ui! { Badge(label = "New", intent = IntentTag::Success, kind = BadgeKind::Soft) }
//! ```

use framework_core::{ui, Primitive, StyleApplication};

use crate::components::button::IntentTag;
use crate::stylesheets::Badge;
use crate::theme::IdeaThemeRef;

/// Visual treatment for a Badge. No Ghost — a borderless transparent
/// chip would be invisible.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum BadgeKind {
    Solid,
    #[default]
    Soft,
    Outlined,
}

impl BadgeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Solid => "solid",
            Self::Soft => "soft",
            Self::Outlined => "outlined",
        }
    }
    pub fn all() -> &'static [BadgeKind] {
        &[BadgeKind::Solid, BadgeKind::Soft, BadgeKind::Outlined]
    }
}

impl framework_core::VariantEnum for BadgeKind {
    fn as_variant_str(self) -> &'static str {
        self.as_str()
    }
    fn all_variants() -> &'static [Self] {
        Self::all()
    }
}

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct BadgeProps {
    pub label: String,
    pub intent: IntentTag,
    pub kind: BadgeKind,
}

impl Default for BadgeProps {
    fn default() -> Self {
        Self {
            label: String::new(),
            // Default to Neutral/Soft — the most generic "this is a
            // status chip" look.
            intent: IntentTag::Neutral,
            kind: BadgeKind::Soft,
        }
    }
}

pub fn badge(props: &BadgeProps) -> Primitive {
    let label = props.label.clone();
    let intent = props.intent;
    let kind = props.kind;
    let appearance = format!("{}_{}", intent.as_str(), kind.as_str());

    let style = move || {
        let _ = framework_core::active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        StyleApplication::new(Badge::sheet()).with("appearance", appearance.clone())
    };

    ui! { Text(style = style) { label } }
}
