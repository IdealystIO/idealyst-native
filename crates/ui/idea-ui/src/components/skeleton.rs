//! `Skeleton` — a muted placeholder block, used to suggest the
//! shape of loading content.
//!
//! Currently static — a tinted rectangle with the configured width,
//! height, and border radius. A pulsing animation would need either
//! a recurring opacity signal driven by the app, or framework
//! support for autonomous looping animations on a style property.
//! For now, a static skeleton still communicates "this region is
//! pending" and avoids layout shift when the real content arrives.
//!
//! ```ignore
//! ui! {
//!     Stack(gap = StackGap::Sm) {
//!         Skeleton(height = 24.0, width = SkeletonWidth::Full)
//!         Skeleton(height = 16.0, width = SkeletonWidth::Full)
//!         Skeleton(height = 16.0, width = SkeletonWidth::Half)
//!     }
//! }
//! ```

use runtime_core::{component, ui, Length, Element, StyleApplication};

use crate::stylesheets::Skeleton as SkeletonStyle;
use crate::theme::IdeaThemeRef;

/// Width preset. Use [`SkeletonWidth::Px`] for an exact pixel width.
#[derive(Copy, Clone, Default)]
pub enum SkeletonWidth {
    /// 100% of the parent.
    #[default]
    Full,
    /// 50% of the parent.
    Half,
    /// 75% of the parent.
    ThreeQuarter,
    /// Exact pixel width.
    Px(f32),
}

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct SkeletonProps {
    pub width: SkeletonWidth,
    pub height: f32,
    /// Border radius in px. `0.0` for a sharp rectangle, larger
    /// values for a pill or circle.
    pub radius: f32,
}

impl Default for SkeletonProps {
    fn default() -> Self {
        Self {
            width: SkeletonWidth::Full,
            height: 16.0,
            radius: 4.0,
        }
    }
}

#[component]
pub fn Skeleton(props: &SkeletonProps) -> Element {
    let height = props.height;
    let radius = props.radius;
    let width = match props.width {
        SkeletonWidth::Full => Length::pct(100.0),
        SkeletonWidth::Half => Length::pct(50.0),
        SkeletonWidth::ThreeQuarter => Length::pct(75.0),
        SkeletonWidth::Px(v) => Length::Px(v),
    };

    let style = move || {
        // Ensure the active theme is wrapped in IdeaThemeRef; the
        // background comes from the stylesheet's base closure
        // against that theme.
        let _ = crate::theme_runtime::active_theme()
            .downcast_ref::<IdeaThemeRef>()
            .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
        let mut app = StyleApplication::new(SkeletonStyle::sheet())
            .override_border_radius(Length::Px(radius));
        // No `override_width` / `override_height` builders yet —
        // poke `overrides` directly. (Framework follow-up.)
        app.overrides.width = Some(runtime_core::Tokenized::Literal(width.clone()));
        app.overrides.height = Some(runtime_core::Tokenized::Literal(Length::Px(height)));
        app
    };

    ui! { View(style = style) {} }
}
