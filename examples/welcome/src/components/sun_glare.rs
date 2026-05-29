//! Sun-glare: a corner-anchored gradient disc that blooms in
//! during Act 2 and breathes for the page lifetime.
//!
//! Wrapper holds the static `translate(50%, -50%)` that pins the
//! disc's centre to the top-right corner; the inner disc holds the
//! per-frame transform animations. Splitting them keeps animated
//! writes from clobbering the centering translate — iOS bakes
//! scale + translate into one `CGAffineTransform`.

use std::rc::Rc;

use runtime_core::{
    component, ui, Color, Gradient, GradientKind, GradientStop, Length, Overflow, Position,
    Element, RadialExtent, StyleRules, StyleSheet, Tokenized, Transform,
};

use crate::coordinator::WelcomeRefs;
use crate::style_helpers::{pct, px, static_sheet};

/// Initial bloom scale. Loose spring up to 1.0 gives an organic
/// spread.
pub const GLARE_INITIAL_SCALE: f32 = 0.55;

/// Anchor size as % of viewport height. Half the disc hangs offscreen
/// (corner anchored), so visible reach is ~half this.
const GLARE_ANCHOR_HEIGHT_PCT: f32 = 60.0;

/// Pulse amplitude. ±8% reads as a clear, organic breath.
pub const SUN_PULSE_AMPLITUDE: f32 = 0.08;

/// Pulse period (ms). Shared by color + scale so warmth and size
/// swell peak together.
pub const SUN_PULSE_PERIOD_MS: f64 = 5200.0;

pub const COLOR_SUN_CORE: &str = "#fff6d8";

// `(dim, bright)` pairs — `sin(t)` maps 0..1 between them. All
// channels in 0..=1 sRGB; alpha is independent.
pub const SUN_CORE_DIM: (f32, f32, f32, f32) = (1.0, 0.95, 0.78, 0.95);
pub const SUN_CORE_BRIGHT: (f32, f32, f32, f32) = (1.0, 0.99, 0.90, 1.00);
pub const SUN_CORONA_DIM: (f32, f32, f32, f32) = (1.0, 0.78, 0.36, 0.70);
pub const SUN_CORONA_BRIGHT: (f32, f32, f32, f32) = (1.0, 0.85, 0.50, 0.95);

#[derive(Default)]
pub struct SunGlareProps {
    pub refs: WelcomeRefs,
}

#[component]
pub fn SunGlare(props: &SunGlareProps) -> Element {
    let refs = props.refs;
    let wrapper = wrapper_sheet();
    let anchor = anchor_sheet();
    ui! {
        View(style = wrapper) {
            View(style = anchor) {}.bind(refs.glare)
        }
    }
}

fn wrapper_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        right: Some(px(0.0)),
        height: Some(pct(GLARE_ANCHOR_HEIGHT_PCT)),
        aspect_ratio: Some(1.0),
        transform: Some(vec![
            Transform::TranslateX(Length::Percent(50.0)),
            Transform::TranslateY(Length::Percent(-50.0)),
        ]),
        ..Default::default()
    })
}

fn anchor_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        right: Some(px(0.0)),
        bottom: Some(px(0.0)),
        left: Some(px(0.0)),
        // 999px = CSS-style "max radius" — each backend clamps to
        // half the smaller side.
        border_top_left_radius: Some(px(999.0)),
        border_top_right_radius: Some(px(999.0)),
        border_bottom_left_radius: Some(px(999.0)),
        border_bottom_right_radius: Some(px(999.0)),
        overflow: Some(Overflow::Hidden),
        opacity: Some(Tokenized::Literal(0.0)),
        background_gradient: Some(Gradient {
            kind: GradientKind::Radial {
                center: (0.5, 0.5),
                radius: 1.0,
                extent: RadialExtent::ClosestSide,
            },
            // Bright core → warm corona → soft halo → transparent.
            // Each ring's alpha is roughly half the previous so the
            // perceived brightness ramp is even.
            stops: vec![
                GradientStop { offset: 0.0, color: Color(COLOR_SUN_CORE.into()) },
                GradientStop { offset: 0.30, color: Color("rgba(255, 210, 110, 0.70)".into()) },
                GradientStop { offset: 0.55, color: Color("rgba(255, 168, 60, 0.22)".into()) },
                GradientStop { offset: 0.80, color: Color("rgba(255, 168, 60, 0.06)".into()) },
                GradientStop { offset: 1.0, color: Color("rgba(255, 168, 60, 0.0)".into()) },
            ],
        }),
        ..Default::default()
    })
}
