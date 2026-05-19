//! Material 3 skin for the native widgets.
//!
//! **TODO** — currently stubs that delegate to the iOS look so
//! apps stay renderable under `SimulatedPlatform::Android` while
//! Material 3 styling is being filled in.
//!
//! Targets when implemented (Material 3 / Material You):
//! - Switch — pill track with circular handle that grows in size
//!   when checked. Track-on uses the primary tonal color,
//!   track-off uses surface-variant.
//! - Slider — thinner track than iOS, square thumb (or
//!   "discrete" variant with tick marks), elevated active state.
//! - TextField — outlined or filled variant with a floating
//!   label, focus indicator line under the field.
//! - All animated with Material's emphasized-decel curves.
//!
//! Implement these by replacing the body of each function below.
//! The signatures match the iOS module's so the dispatch in
//! `super::paint_*` doesn't have to change.

use glyphon::Buffer;

use crate::pipeline::Instance as RectInstance;
use crate::text::StagedText;

use super::ios;

pub fn paint_toggle(x: f32, y: f32, w: f32, h: f32, t: f32, rects: &mut Vec<RectInstance>) {
    // TODO(android): Material 3 switch. For now: iOS fallback so
    // the simulator stays functional under `Android` profile.
    ios::paint_toggle(x, y, w, h, t, rects);
}

pub fn paint_slider(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    value: f32,
    min: f32,
    max: f32,
    rects: &mut Vec<RectInstance>,
) {
    // TODO(android): Material 3 slider.
    ios::paint_slider(x, y, w, h, value, min, max, rects);
}

#[allow(clippy::too_many_arguments)]
pub fn paint_text_input<'a>(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    is_focused: bool,
    draw_caret: bool,
    is_placeholder: bool,
    buffer: &'a Buffer,
    caret_x_local: f32,
    text_color: [f32; 4],
    rects: &mut Vec<RectInstance>,
    texts: &mut Vec<StagedText<'a>>,
) {
    // TODO(android): Material 3 outlined / filled text field.
    ios::paint_text_input(
        x,
        y,
        w,
        h,
        is_focused,
        draw_caret,
        is_placeholder,
        buffer,
        caret_x_local,
        text_color,
        rects,
        texts,
    );
}
