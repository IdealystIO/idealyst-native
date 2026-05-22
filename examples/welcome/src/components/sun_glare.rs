//! Sun-glare anchor + animated disc that blooms in from the top-
//! right corner during Act 2.
//!
//! Two stylesheets:
//! - [`glare_wrapper_sheet`] carries the static corner-centering
//!   transform.
//! - [`glare_anchor_sheet`] is the animated disc inside it (opacity,
//!   scale, gradient stop pulse).
//!
//! Splitting them keeps the per-frame transform writes from
//! clobbering the static `translate(50%, -50%)` that pins the
//! wrapper to the corner — iOS in particular bakes scale + translate
//! into a single `CGAffineTransform`, so any animated write to the
//! disc-with-translate would otherwise overwrite the centering
//! offset.

use std::rc::Rc;

use framework_core::{
    Color, Gradient, GradientKind, GradientStop, Length, Overflow, Position, RadialExtent,
    StyleRules, StyleSheet, Tokenized, Transform,
};

use crate::style_helpers::{pct, px, static_sheet};

/// Initial scale of the sun-glare disc — starts as a tight point of
/// light and a loose spring (low stiffness, moderate damping) carries
/// it up to its resting size, giving the bloom a slow, organic
/// spread.
pub const GLARE_INITIAL_SCALE: f32 = 0.55;

/// Sun-glare anchor size as a fraction of viewport HEIGHT. The
/// stylesheet pairs this with `aspect_ratio: 1.0` so the box stays
/// square; the layout engine derives width from height. Height-
/// relative ties the sun's apparent size to the vertical extent
/// of the screen — feels more grounded than width-relative, where
/// a wide landscape window would push the sun comically large.
///
/// Note: the wrapper sits at the top-right corner with
/// `translate(50%, -50%)`, so only the bottom-left **quadrant** of
/// the disc is on-screen. Effective visible reach is therefore
/// roughly `height_pct / 2` along each axis — `60%` means the
/// bloom extends ~30% of viewport height into the page, which
/// reads as a hero light source rather than a decorative dot.
const GLARE_ANCHOR_HEIGHT_PCT: f32 = 60.0;

/// Sun-glare breathe amplitude. The raf-driven pulse adds
/// `sin(t) * amp` to the resting scale of 1.0, so the sun throbs
/// between `1 - amp` and `1 + amp`. ±8% reads as a clear, organic
/// breath; larger feels gimmicky, smaller is invisible.
pub const SUN_PULSE_AMPLITUDE: f32 = 0.08;

/// Period of the sun's color + scale breathe, in milliseconds. Both
/// the per-stop color animator and the scale animator share this so
/// the warmth swell and the size swell stay phase-locked. ~5 s reads
/// as an unhurried, alive presence; faster starts to feel anxious.
pub const SUN_PULSE_PERIOD_MS: f64 = 5200.0;

/// Sun-glare core — near-white with the faintest warmth.
pub const COLOR_SUN_CORE: &str = "#fff6d8";

// ---- Pulse palette -------------------------------------------------------
//
// Two-color cycles for the raf-driven pulse. Each pair is `(dim,
// bright)` — `sin(t)` maps `0..1` between them. Sticking with the
// warm-gold family on both ends keeps the pulse breathing rather
// than oscillating between two visibly different hues. Channels
// are 0..=1 sRGB; alpha is independent.
pub const SUN_CORE_DIM: (f32, f32, f32, f32) = (1.0, 0.95, 0.78, 0.95);
pub const SUN_CORE_BRIGHT: (f32, f32, f32, f32) = (1.0, 0.99, 0.90, 1.00);
pub const SUN_CORONA_DIM: (f32, f32, f32, f32) = (1.0, 0.78, 0.36, 0.70);
pub const SUN_CORONA_BRIGHT: (f32, f32, f32, f32) = (1.0, 0.85, 0.50, 0.95);

/// Positioned wrapper for the sun. Pinned to the viewport's
/// top-right edge then translated by HALF its own dimensions on
/// each axis — `translate(50%, -50%)` is BOX-relative in CSS, so
/// the shift is always exactly half the wrapper's size on any
/// device. Half the disc hangs offscreen by design: the radial
/// gradient reads as a light source cresting through the top-right
/// corner rather than as a visible circle.
///
/// The wrapper carries ONLY the static layout / static transform.
/// The animated disc lives inside it (see [`glare_anchor_sheet`])
/// so per-frame writes to scale don't clobber this translate —
/// iOS in particular bakes scale + translate into a single
/// `CGAffineTransform`, and any animated write would otherwise
/// overwrite our centering offset.
pub fn glare_wrapper_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        right: Some(px(0.0)),
        // Size by HEIGHT (aspect_ratio:1.0 makes the box square,
        // deriving width from height). Reads as a screen-vertical
        // anchor — taller phone = bigger sun.
        height: Some(pct(GLARE_ANCHOR_HEIGHT_PCT)),
        aspect_ratio: Some(1.0),
        transform: Some(vec![
            Transform::TranslateX(Length::Percent(50.0)),
            Transform::TranslateY(Length::Percent(-50.0)),
        ]),
        ..Default::default()
    })
}

/// Inner disc: fills the wrapper, holds the gradient, takes the
/// per-frame opacity + scale + stop-color animations. Scale
/// animation pivots from the disc's own center (default
/// transform-origin), which the wrapper has placed exactly on
/// the viewport's top-right corner — so the pulse "breathes
/// from the corner" without any per-axis math here.
pub fn glare_anchor_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        // Fill the wrapper.
        top: Some(px(0.0)),
        right: Some(px(0.0)),
        bottom: Some(px(0.0)),
        left: Some(px(0.0)),
        // Clip to a perfect circle. CSS-style "max radius" — each
        // backend clamps to half the smaller side (iOS's
        // `apply_style_to_view` handles this explicitly because
        // UIKit's `cornerRadius` doesn't clamp on its own).
        border_top_left_radius: Some(px(999.0)),
        border_top_right_radius: Some(px(999.0)),
        border_bottom_left_radius: Some(px(999.0)),
        border_bottom_right_radius: Some(px(999.0)),
        // `Overflow::Hidden` ensures the gradient sublayer is clipped
        // to the rounded corner. iOS's cornerRadius path already sets
        // `clipsToBounds=true`, but stating it explicitly here makes
        // the circle behavior an author intent visible at the call
        // site (and protects against future backend changes that
        // might decouple radius from clipping).
        overflow: Some(Overflow::Hidden),
        opacity: Some(Tokenized::Literal(0.0)),
        // Radial gradient: bright cream core → warm gold corona →
        // soft orange halo → transparent edge. The transparent
        // outermost stop produces the soft falloff that used to
        // require stacked partial-alpha discs.
        background_gradient: Some(Gradient {
            kind: GradientKind::Radial {
                center: (0.5, 0.5),
                radius: 1.0,
                // The sun's anchor is aspect-ratio:1 (a square),
                // so ClosestSide puts the transparent edge stop
                // exactly at the view boundary — the gradient
                // fills the whole circular clip.
                extent: RadialExtent::ClosestSide,
            },
            // Four-stop falloff tuned for the larger anchor: bright
            // cream core kept tight (offset 0–0.18) so the hot
            // center reads as a sun, then a long, mostly-transparent
            // tail that fades out gently to the edge of the disc.
            // Each ring's alpha is roughly half the previous, which
            // gives a perceptually-even brightness ramp (alpha is
            // gamma-space additive, the eye expects exponential
            // falloff for a "smooth" gradient).
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: Color(COLOR_SUN_CORE.into()),
                },
                GradientStop {
                    offset: 0.30,
                    color: Color("rgba(255, 210, 110, 0.70)".into()),
                },
                GradientStop {
                    offset: 0.55,
                    color: Color("rgba(255, 168, 60, 0.22)".into()),
                },
                GradientStop {
                    offset: 0.80,
                    color: Color("rgba(255, 168, 60, 0.06)".into()),
                },
                GradientStop {
                    offset: 1.0,
                    color: Color("rgba(255, 168, 60, 0.0)".into()),
                },
            ],
        }),
        ..Default::default()
    })
}
