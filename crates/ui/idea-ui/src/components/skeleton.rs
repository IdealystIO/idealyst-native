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

use runtime_core::{component, component_styles, ui, Element, IdealystSchema, Length, Reactive, StyleRules};

use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Skeleton`].
    pub Skeleton<IdeaThemeRef> {
        slot root {
            base(t) {
                background: t.color.surface_alt(),
            }
            transitions {
                background: 250ms EaseInOut,
            }
        }
    }
}

/// Width preset. Use [`SkeletonWidth::Px`] for an exact pixel width.
#[derive(Copy, Clone, Default, runtime_core::Remote)]
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
#[runtime_core::props(styles = SkeletonStyles)]
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
            styles: Default::default(),
        }
    }
}

/// Renders a muted, fixed-size placeholder block to reserve space for
/// loading content (no animation; avoids layout shift on arrival).
#[component]
pub fn Skeleton(props: &SkeletonProps) -> Element {
    // Per-instance dimensions + radius ride the INLINE layer: they are
    // continuous values drawn from props, so as overrides they minted a
    // class per (w, h, r) and kept every Skeleton on the live engine. The
    // slot's own fill premints.
    let dims = |width: SkeletonWidth, height: f32, radius: f32| {
        let length = match width {
            SkeletonWidth::Full => Length::pct(100.0),
            SkeletonWidth::Half => Length::pct(50.0),
            SkeletonWidth::ThreeQuarter => Length::pct(75.0),
            SkeletonWidth::Px(v) => Length::Px(v),
        };
        let r = runtime_core::Tokenized::Literal(Length::Px(radius));
        StyleRules {
            width: Some(runtime_core::Tokenized::Literal(length)),
            height: Some(runtime_core::Tokenized::Literal(Length::Px(height))),
            border_top_left_radius: Some(r.clone()),
            border_top_right_radius: Some(r.clone()),
            border_bottom_left_radius: Some(r.clone()),
            border_bottom_right_radius: Some(r),
            ..Default::default()
        }
    };
    let style = SkeletonStyles::root();
    let style = match (props.width.clone(), props.height.clone(), props.radius.clone()) {
        (Reactive::Static(w), Reactive::Static(h), Reactive::Static(r)) => style.inline(dims(w, h, r)),
        (w, h, r) => style.inline_with(move || dims(w.get(), h.get(), r.get())),
    };
    let style = style.overridden(props.styles.root.as_ref());
    ui! { view(style = style) {} }
}
