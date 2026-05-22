//! Three planets orbit the sun on tight elliptical paths sized to
//! pass across the welcome text in the middle of the viewport. Each
//! planet is rendered TWICE — once before the content layer (back),
//! once after (front) — sharing position AVs but with opposite-
//! phase opacity AVs. The raf-driver writes
//! `opacity = max(sin θ, 0)` on the front view and
//! `max(-sin θ, 0)` on the back view, so a single planet "passes
//! behind" the text when its orbit angle is in the upper half of
//! the circle (sin < 0) and "in front of" when in the lower half.
//! The z-swap is the whole point of the duplication — the framework
//! doesn't have a per-frame z-index animation, but document-order
//! render + opacity flip-flop is equivalent.

use std::rc::Rc;

use framework_core::{
    Color, Gradient, GradientKind, GradientStop, Position, RadialExtent, StyleRules, StyleSheet,
    Tokenized,
};

use crate::style_helpers::{px, static_sheet};

/// Per-planet config. `rx_frac` / `ry_frac` are fractions of viewport
/// width / height for the elliptical semi-axes (orbit center is the
/// top-right corner, where the sun lives). `period_ms` is one full
/// revolution; `phase_offset` (radians) staggers the three planets
/// so they don't all line up. `size_dp` is the dot diameter;
/// `color` is a CSS string.
pub struct PlanetConfig {
    pub rx_frac: f32,
    pub ry_frac: f32,
    pub period_ms: f64,
    pub phase_offset: f32,
    pub size_dp: f32,
    /// Solid body colour — no gradient / specular. The darken
    /// overlay (driven by distance-from-sun) supplies the
    /// shading cue without needing a multi-stop gradient.
    pub color: &'static str,
}

/// Three planets at increasing radii / decreasing speeds — closer
/// to the sun = faster, like real Keplerian orbits. The middle
/// planet's orbit (`rx ≈ 0.5 * vw`, `ry ≈ 0.5 * vh`) passes
/// directly through the welcome text's bbox, so the z-swap is most
/// visible there. The inner planet is small + tight + fast; the
/// outer is larger + wider + slow, sweeping past the bottom-left
/// corner once every ~20 s.
pub const PLANETS: [PlanetConfig; 3] = [
    // Each planet orbits the welcome text (NOT the sun) on a
    // diagonally-tilted plane. `rx_frac` is the 2D ellipse's
    // semi-major (along the diagonal) as fraction of viewport
    // WIDTH; `ry_frac` is the semi-minor (perpendicular) as
    // fraction of viewport HEIGHT. The tilt is implicit in the
    // 2D vs depth axes — the same `sin(θ)` that swings the
    // planet along the minor axis ALSO drives its depth, so
    // every planet visit along the orbit has a depth that
    // matches the perpendicular sway. Combined with the
    // scale animation (`small back ↔ large front`), this reads
    // as a real 3D circle viewed from above the orbit plane.
    // All three orbits share the centre (viewport centre) and
    // the 45° diagonal major axis; they differ in size + speed.
    // `ry_frac` × vh sizes the diagonal MAJOR semi-axis; the
    // major direction is at 45° so the orbit's vertical reach
    // is `ry_frac × vh / √2`. To extend past the welcome text
    // (which roughly spans the middle 30% of the viewport
    // vertically), the middle planet uses `ry_frac ≈ 0.45` →
    // vertical reach ≈ 0.32 × vh, clearing the text by ~17%
    // on each side.
    // For a 45° diagonal major axis, a unit of `r_major`
    // contributes 1/√2 ≈ 0.707 to BOTH the horizontal and
    // vertical reach. So if `r_major = 0.30 × vh` on a 393×852
    // viewport, the orbit spans ±0.21 × vh ≈ ±180 px in both
    // x and y from the centre — fits horizontally (vw/2 = 197)
    // and clears the text vertically (text band ≈ 350-540, so
    // half-text = 95 px). Bigger `r_major` lets the diagonal
    // extremes hang off the left/right edges (intentional for
    // the outer planet).
    // Constraint: for a 45° diagonal major axis, the orbit's
    // worst-case horizontal offset from the centre is
    // `(1/√2) × sqrt(r_major² + r_minor²)`. To stay inside the
    // viewport horizontally (|offset_x| ≤ vw/2 ≈ 197 on a
    // typical phone) the bounding-circle radius
    // `sqrt(r_major² + r_minor²)` must stay below `vw/2 × √2`
    // ≈ 0.71 × vw. Sizes below honour this so all three orbits
    // sit inside the viewport.
    PlanetConfig {
        // Inner — soft, dusty terracotta / muted rose.
        rx_frac: 0.08,  // semi-MINOR (perp to diagonal) as frac of vh
        ry_frac: 0.18,  // semi-MAJOR (along diagonal) as frac of vh
        period_ms: 8000.0,
        phase_offset: 0.0,
        size_dp: 14.0,
        color: "#c89580",
    },
    PlanetConfig {
        // Middle — pale sage / muted olive.
        rx_frac: 0.11,
        ry_frac: 0.24,
        period_ms: 13000.0,
        phase_offset: 2.09,
        size_dp: 22.0,
        color: "#b6c293",
    },
    PlanetConfig {
        // Outer — dusty blue-gray. Slowest period gives a
        // stately, gravitational feel.
        rx_frac: 0.14,
        ry_frac: 0.30,
        period_ms: 20000.0,
        phase_offset: 4.18,
        size_dp: 18.0,
        color: "#9aafc0",
    },
];

/// Scale at the orbit's back extreme (depth = -1). Sub-1.0 so
/// the planet visibly shrinks when it's "behind" the welcome
/// text. The contrast against `PLANET_SCALE_FRONT` is the main
/// depth cue in the 3D illusion.
pub const PLANET_SCALE_BACK: f32 = 0.45;

/// Scale at the orbit's front extreme (depth = +1). >1.0 so
/// the planet visibly grows when it's "in front of" the text.
pub const PLANET_SCALE_FRONT: f32 = 1.55;

/// How long the planets take to fade in from invisible once the
/// raf-driver starts running (= Act 2 + 200 ms, same as sun
/// bloom). Without a fade-in, the planet whose `phase_offset`
/// puts it on the lower half at t=0 would pop on at non-zero
/// alpha; this ramps the whole system up smoothly.
pub const PLANET_FADE_IN_MS: f64 = 1500.0;

/// One planet body — a circle with a radial gradient whose
/// bright "lit" centre is offset toward the upper-right (the
/// direction of the sun corner). The shaded side fades to the
/// `color_dark` body tone. Two copies of each planet are
/// rendered (before + after the content layer) so the raf-
/// driver can swap z-order via opposite-phase opacity AVs.
pub fn planet_sheet(size_dp: f32, color: &'static str) -> Rc<StyleSheet> {
    // Build the "almost-transparent edge" colour from the
    // solid body colour by appending `00` alpha. CSS / iOS /
    // Android all accept 8-digit hex (`#rrggbbaa`) as a
    // shorthand for a colour with alpha.
    let edge_color = format!("{}00", color);
    let mid_color = format!("{}cc", color);
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        right: Some(px(0.0)),
        width: Some(px(size_dp)),
        height: Some(px(size_dp)),
        // No solid background — the gradient supplies both the
        // body colour and the soft falloff to transparent at
        // the edge.
        background_gradient: Some(Gradient {
            kind: GradientKind::Radial {
                center: (0.5, 0.5),
                radius: 1.0,
                extent: RadialExtent::ClosestSide,
            },
            stops: vec![
                // Solid body for the inner ~60%.
                GradientStop {
                    offset: 0.0,
                    color: Color(color.into()),
                },
                GradientStop {
                    offset: 0.60,
                    color: Color(color.into()),
                },
                // Quick mid-stop holds most of the alpha to
                // 80% radius, so the soft edge is the OUTER
                // 20% of the disc — a planet with a halo
                // rather than a fuzzy ball.
                GradientStop {
                    offset: 0.80,
                    color: Color(mid_color.into()),
                },
                GradientStop {
                    offset: 1.0,
                    color: Color(edge_color.into()),
                },
            ],
        }),
        // Start invisible — the raf-driver writes the real
        // opacity (one half of `sin θ`) once it begins running
        // at Act 2 + 200 ms.
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    })
}
