//! Full-page vignette overlay — a rounded-rectangle warm-yellow
//! glow around the frame edges with a transparent center. Built from
//! four child edge bands (one per side); where two bands overlap in
//! a corner their alphas add, so corners come out a touch brighter,
//! which reads correctly as "the light is strongest at the corners."
//!
//! The wrapper carries the Act 2 opacity tween (0 → 1). Each band's
//! outer warm stop alpha is then pulsed by the unified raf-driver in
//! [`crate::app`] so the glow breathes in lock-step with the sun.

use std::rc::Rc;

use framework_core::{
    Color, Gradient, GradientKind, GradientStop, Position, StyleRules, StyleSheet, Tokenized,
};

use crate::style_helpers::{pct, px, static_sheet};

// ---- Pulse palette -------------------------------------------------------
//
// Alpha range: the vignette is supposed to read as ambient warmth at
// the very edge of the frame. Each edge band peaks at these alphas;
// where two bands overlap in a corner the effective alpha doubles.

pub const VIGNETTE_CORNER_DIM: (f32, f32, f32, f32) = (1.0, 0.78, 0.36, 0.015);
pub const VIGNETTE_CORNER_BRIGHT: (f32, f32, f32, f32) = (1.0, 0.85, 0.50, 0.06);

/// Cross-axis depth of each vignette band as a fraction of the
/// containing viewport. The band is fully transparent at its
/// inner edge and ramps to the warm color at the screen edge.
/// Smaller values keep the glow hugging the very perimeter; the
/// dark interior stays clean.
const VIGNETTE_BAND_PCT: f32 = 28.0;

/// Vignette wrapper — full-page transparent container. Just carries
/// the opacity animation; the actual warm glow comes from the four
/// child band views (see [`vignette_band_sheet`]).
pub fn vignette_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        left: Some(px(0.0)),
        right: Some(px(0.0)),
        bottom: Some(px(0.0)),
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    })
}

/// Which screen edge a vignette band hugs. The band's gradient
/// runs perpendicular to the edge, fading from "warm at the edge"
/// inward to "fully transparent."
#[derive(Clone, Copy)]
pub enum VignetteEdge {
    Top,
    Bottom,
    Left,
    Right,
}

/// One edge band of the rounded-box vignette. Pinned to one
/// screen edge with a `VIGNETTE_BAND_PCT` cross-axis depth. Linear
/// gradient runs from transparent (inner edge) to warm (screen
/// edge); the warm stop's alpha is pulsed by the raf-driver.
pub fn vignette_band_sheet(edge: VignetteEdge) -> Rc<StyleSheet> {
    // Angle convention: `0deg` = bottom→top, so stop at offset 0
    // sits at the BOTTOM of the gradient axis and stop at offset 1
    // at the TOP. For each band we want the warm (stop 1) end at
    // the screen edge:
    // - Top band: warm at the top of its own box → angle 0deg.
    // - Bottom band: warm at the bottom → angle 180deg.
    // - Left band: warm at the left → angle 270deg.
    // - Right band: warm at the right → angle 90deg.
    let (top, bottom, left, right, width, height, angle_deg) = match edge {
        VignetteEdge::Top => (
            Some(px(0.0)),
            None,
            Some(px(0.0)),
            Some(px(0.0)),
            None,
            Some(pct(VIGNETTE_BAND_PCT)),
            0.0_f32,
        ),
        VignetteEdge::Bottom => (
            None,
            Some(px(0.0)),
            Some(px(0.0)),
            Some(px(0.0)),
            None,
            Some(pct(VIGNETTE_BAND_PCT)),
            180.0,
        ),
        VignetteEdge::Left => (
            Some(px(0.0)),
            Some(px(0.0)),
            Some(px(0.0)),
            None,
            Some(pct(VIGNETTE_BAND_PCT)),
            None,
            270.0,
        ),
        VignetteEdge::Right => (
            Some(px(0.0)),
            Some(px(0.0)),
            None,
            Some(px(0.0)),
            Some(pct(VIGNETTE_BAND_PCT)),
            None,
            90.0,
        ),
    };
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top,
        bottom,
        left,
        right,
        width,
        height,
        background_gradient: Some(Gradient {
            kind: GradientKind::Linear { angle_deg },
            // `[transparent, warm]` — the pulse driver writes
            // index 1 (the "warm" stop) every frame; index 0
            // stays fully transparent so the band fades out into
            // the page interior smoothly.
            stops: vec![
                GradientStop { offset: 0.0, color: Color("rgba(255, 168, 60, 0.0)".into()) },
                GradientStop { offset: 1.0, color: Color("rgba(255, 168, 60, 0.0)".into()) },
            ],
        }),
        ..Default::default()
    })
}
