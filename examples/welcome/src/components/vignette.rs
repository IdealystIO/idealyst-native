//! Full-page rounded-rectangle vignette — a warm-yellow glow at the
//! frame edges with a transparent interior. Built from four child
//! edge bands; overlapping corners read brighter, matching the
//! "light strongest at the corners" reading. The wrapper carries
//! the Act 2 opacity tween; each band's warm stop is then pulsed
//! by the raf-driver in [`crate::coordinator`] in lock-step with
//! the sun.

use std::rc::Rc;

use runtime_core::{
    component, ui, Color, Gradient, GradientKind, GradientStop, Position, Element, StyleRules,
    StyleSheet, Tokenized,
};

use crate::coordinator::WelcomeRefs;
use crate::style_helpers::{pct, px, static_sheet};

// `(dim, bright)` pairs for the raf-driven pulse on each band's warm
// stop. Where two bands overlap in a corner the alpha doubles.
pub const VIGNETTE_CORNER_DIM: (f32, f32, f32, f32) = (1.0, 0.78, 0.36, 0.015);
pub const VIGNETTE_CORNER_BRIGHT: (f32, f32, f32, f32) = (1.0, 0.85, 0.50, 0.06);

/// Cross-axis depth of each band as a fraction of viewport. Smaller
/// = glow hugs the perimeter more tightly.
const VIGNETTE_BAND_PCT: f32 = 28.0;

pub struct VignetteProps {
    pub refs: WelcomeRefs,
}

#[component]
pub fn Vignette(props: &VignetteProps) -> Element {
    let refs = props.refs;
    let wrapper = wrapper_sheet();
    let top = band_sheet(Edge::Top);
    let bottom = band_sheet(Edge::Bottom);
    let left = band_sheet(Edge::Left);
    let right = band_sheet(Edge::Right);
    ui! {
        View(style = wrapper) {
            View(style = top) {}.bind(refs.vignette_top)
            View(style = bottom) {}.bind(refs.vignette_bottom)
            View(style = left) {}.bind(refs.vignette_left)
            View(style = right) {}.bind(refs.vignette_right)
        }.bind(refs.vignette)
    }
}

fn wrapper_sheet() -> Rc<StyleSheet> {
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

#[derive(Clone, Copy)]
enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

fn band_sheet(edge: Edge) -> Rc<StyleSheet> {
    // Gradient angle convention: `0deg` = bottom→top, so we point
    // the warm (stop 1) end at the screen edge per band.
    let (top, bottom, left, right, width, height, angle_deg) = match edge {
        Edge::Top => (
            Some(px(0.0)), None, Some(px(0.0)), Some(px(0.0)),
            None, Some(pct(VIGNETTE_BAND_PCT)), 0.0_f32,
        ),
        Edge::Bottom => (
            None, Some(px(0.0)), Some(px(0.0)), Some(px(0.0)),
            None, Some(pct(VIGNETTE_BAND_PCT)), 180.0,
        ),
        Edge::Left => (
            Some(px(0.0)), Some(px(0.0)), Some(px(0.0)), None,
            Some(pct(VIGNETTE_BAND_PCT)), None, 270.0,
        ),
        Edge::Right => (
            Some(px(0.0)), Some(px(0.0)), None, Some(px(0.0)),
            Some(pct(VIGNETTE_BAND_PCT)), None, 90.0,
        ),
    };
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top, bottom, left, right, width, height,
        background_gradient: Some(Gradient {
            kind: GradientKind::Linear { angle_deg },
            // Both stops start transparent; the raf-driver writes
            // the warm color into stop 1 each frame.
            stops: vec![
                GradientStop { offset: 0.0, color: Color("rgba(255, 168, 60, 0.0)".into()) },
                GradientStop { offset: 1.0, color: Color("rgba(255, 168, 60, 0.0)".into()) },
            ],
        }),
        ..Default::default()
    })
}
