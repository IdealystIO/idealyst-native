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

use runtime_core::{component, ui, IdealystSchema, Length, Element, Reactive, StyleApplication};

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

// Reactive-by-default: `#[props]` wraps each scalar-DATA field `T` →
// `Reactive<T>`. All three (width/height/radius) drive the placeholder's
// style, so they route into the style sink; a bare value stays a zero-cost
// `Static` snapshot (the no-flicker fast path), a `Signal`/`rx!` re-styles
// the block in place.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct SkeletonProps {
    /// Width preset (Full/Half/ThreeQuarter) or exact pixels. Default Full.
    pub width: SkeletonWidth,
    /// Block height in px. Default 16.
    #[schema(constraint = "pixels, > 0")]
    pub height: f32,
    /// Border radius in px. `0.0` for a sharp rectangle, larger
    /// values for a pill or circle.
    #[schema(constraint = "pixels, >= 0")]
    pub radius: f32,
}

impl Default for SkeletonProps {
    fn default() -> Self {
        Self {
            width: Reactive::Static(SkeletonWidth::Full),
            height: Reactive::Static(16.0),
            radius: Reactive::Static(4.0),
        }
    }
}

/// Renders a muted, fixed-size placeholder block to reserve space for
/// loading content (no animation; avoids layout shift on arrival).
#[component]
pub fn Skeleton(props: &SkeletonProps) -> Element {
    // The style is REACTIVE when any dimension prop is live; otherwise it's
    // the build-time fast path. The closure reads each prop's `.get()` INSIDE
    // so the apply-style Effect subscribes to whichever are dynamic.
    let style_is_reactive =
        !props.width.is_static() || !props.height.is_static() || !props.radius.is_static();

    let make_style = {
        let width = props.width.clone();
        let height = props.height.clone();
        let radius = props.radius.clone();
        move || {
            let length = match width.get() {
                SkeletonWidth::Full => Length::pct(100.0),
                SkeletonWidth::Half => Length::pct(50.0),
                SkeletonWidth::ThreeQuarter => Length::pct(75.0),
                SkeletonWidth::Px(v) => Length::Px(v),
            };
            // Ensure the active theme is wrapped in IdeaThemeRef; the
            // background comes from the stylesheet's base closure
            // against that theme.
            let _ = crate::theme_runtime::active_theme_untracked()
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed — call install_idea_theme(...) first");
            let mut app = StyleApplication::new(SkeletonStyle::sheet())
                .override_border_radius(Length::Px(radius.get()));
            // No `override_width` / `override_height` builders yet —
            // poke `overrides` directly. (Framework follow-up.)
            app.overrides.width = Some(runtime_core::Tokenized::Literal(length));
            app.overrides.height = Some(runtime_core::Tokenized::Literal(Length::Px(height.get())));
            app
        }
    };

    if style_is_reactive {
        ui! { view(style = make_style) {} }
    } else {
        ui! { view(style = make_style()) {} }
    }
}
