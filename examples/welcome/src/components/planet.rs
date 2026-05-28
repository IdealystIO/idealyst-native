//! Three planets orbit screen-centre on a 45° diagonal ellipse,
//! z-swapping above/below the content layer via an animated
//! z-index (one view per planet, no duplication). Each `Planet`
//! invocation renders a single orbit; the parent loops `0..3` to
//! mount all three.

use std::rc::Rc;

use runtime_core::{
    component, ui, Color, Gradient, GradientKind, GradientStop, Position, Primitive, RadialExtent,
    StyleRules, StyleSheet, Tokenized,
};

use crate::coordinator::WelcomeRefs;
use crate::style_helpers::{px, static_sheet};

/// `rx_frac` / `ry_frac` are the orbit's semi-axes as fractions of
/// viewport HEIGHT (height-relative on both so portrait/landscape
/// rotations preserve orbit aspect ratio). `phase_offset` (radians)
/// staggers the three planets so they don't line up.
pub struct PlanetConfig {
    pub rx_frac: f32,
    pub ry_frac: f32,
    pub period_ms: f64,
    pub phase_offset: f32,
    pub size_dp: f32,
    pub color: &'static str,
}

pub const PLANETS: [PlanetConfig; 3] = [
    // Inner — terracotta. Small + tight + fast.
    PlanetConfig {
        rx_frac: 0.08,
        ry_frac: 0.18,
        period_ms: 8000.0,
        phase_offset: 0.0,
        size_dp: 14.0,
        color: "#c89580",
    },
    // Middle — sage. Largest + crosses through the welcome text.
    PlanetConfig {
        rx_frac: 0.11,
        ry_frac: 0.24,
        period_ms: 13000.0,
        phase_offset: 2.09,
        size_dp: 22.0,
        color: "#b6c293",
    },
    // Outer — dusty blue. Slow stately period.
    PlanetConfig {
        rx_frac: 0.14,
        ry_frac: 0.30,
        period_ms: 20000.0,
        phase_offset: 4.18,
        size_dp: 18.0,
        color: "#9aafc0",
    },
];

/// Scale at depth = −1 (back of orbit). Sub-1.0 so the planet
/// visibly shrinks behind the welcome text — main depth cue.
pub const PLANET_SCALE_BACK: f32 = 0.45;

/// Scale at depth = +1 (front of orbit).
pub const PLANET_SCALE_FRONT: f32 = 1.55;

/// Fade-in for the planet system once the raf-driver starts (Act 2
/// + 200 ms). Without it, planets whose phase puts them on the
/// lower-half arc at t=0 would pop on at non-zero alpha.
pub const PLANET_FADE_IN_MS: f64 = 1500.0;

pub struct PlanetProps {
    pub idx: usize,
    pub refs: WelcomeRefs,
}

#[component]
pub fn Planet(props: &PlanetProps) -> Primitive {
    let cfg = &PLANETS[props.idx];
    let sheet = sheet(cfg.size_dp, cfg.color);
    let target = props.refs.planets[props.idx];
    ui! {
        View(style = sheet) {}.bind(target)
    }
}

fn sheet(size_dp: f32, color: &'static str) -> Rc<StyleSheet> {
    // CSS/iOS/Android all accept 8-digit hex `#rrggbbaa` so we can
    // synthesise the soft-edge falloff colours from the body colour.
    let edge_color = format!("{}00", color);
    let mid_color = format!("{}cc", color);
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        right: Some(px(0.0)),
        width: Some(px(size_dp)),
        height: Some(px(size_dp)),
        background_gradient: Some(Gradient {
            kind: GradientKind::Radial {
                center: (0.5, 0.5),
                radius: 1.0,
                extent: RadialExtent::ClosestSide,
            },
            stops: vec![
                GradientStop { offset: 0.0, color: Color(color.into()) },
                GradientStop { offset: 0.60, color: Color(color.into()) },
                GradientStop { offset: 0.80, color: Color(mid_color.into()) },
                GradientStop { offset: 1.0, color: Color(edge_color.into()) },
            ],
        }),
        // Raf-driver writes the real opacity once Act 2 starts.
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    })
}
