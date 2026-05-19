//! iOS / UIKit skin for the native widgets.
//!
//! Matches Apple's `UISwitch`, `UISlider`, and `UITextField`:
//! pill-shaped tracks, white circular thumbs with soft drop
//! shadows, blue accent (`systemBlue`), green ON-track
//! (`systemGreen`), translucent gray OFF-track
//! (`systemGray5`), hairline gray field borders.

use glyphon::{Buffer, TextBounds};

use crate::animation::lerp_color;
use crate::node::{
    SLIDER_THUMB_SIZE, SLIDER_TRACK_HEIGHT, TEXT_INPUT_CARET_WIDTH, TOGGLE_THUMB_INSET,
};
use crate::pipeline::Instance as RectInstance;
use crate::text::StagedText;

use super::{rect_inst, TEXT_INPUT_HPAD, TEXT_INPUT_VPAD};

// ---------------------------------------------------------------------------
// iOS palette (mirrors UIKit's system colors)
// ---------------------------------------------------------------------------

const IOS_BLUE: [f32; 4] = [0.0, 0x7a as f32 / 255.0, 1.0, 1.0]; // #007AFF
const IOS_GREEN: [f32; 4] = [
    0x34 as f32 / 255.0,
    0xC7 as f32 / 255.0,
    0x59 as f32 / 255.0,
    1.0,
]; // #34C759
const IOS_TRACK_OFF: [f32; 4] = [
    0x78 as f32 / 255.0,
    0x78 as f32 / 255.0,
    0x80 as f32 / 255.0,
    0.32,
]; // systemGray w/ alpha
const IOS_THUMB: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const IOS_THUMB_SHADOW: [f32; 4] = [0.0, 0.0, 0.0, 0.12];
const IOS_TEXT_BORDER: [f32; 4] = [
    0xC7 as f32 / 255.0,
    0xC7 as f32 / 255.0,
    0xCC as f32 / 255.0,
    1.0,
]; // systemGray3
const IOS_TEXT_INPUT_BG: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const IOS_PLACEHOLDER: [f32; 4] = [60.0 / 255.0, 60.0 / 255.0, 67.0 / 255.0, 0.6];

// ---------------------------------------------------------------------------
// Toggle — UISwitch
// ---------------------------------------------------------------------------

/// 51×31 pill with white circular thumb. Track color blends from
/// gray to green as `t` moves 0 → 1; thumb position linearly
/// interpolates between the OFF and ON anchors.
pub fn paint_toggle(x: f32, y: f32, w: f32, h: f32, t: f32, rects: &mut Vec<RectInstance>) {
    let t = t.clamp(0.0, 1.0);
    let track_radius = h * 0.5;
    let track_color = lerp_color(IOS_TRACK_OFF, IOS_GREEN, t);

    rects.push(rect_inst(
        x,
        y,
        w,
        h,
        track_color,
        [track_radius; 4],
        [0.0; 4],
        0.0,
    ));

    let diameter = h - TOGGLE_THUMB_INSET * 2.0;
    let thumb_x_off = x + TOGGLE_THUMB_INSET;
    let thumb_x_on = x + w - TOGGLE_THUMB_INSET - diameter;
    let thumb_x = thumb_x_off + (thumb_x_on - thumb_x_off) * t;
    let thumb_y = y + TOGGLE_THUMB_INSET;

    rects.push(rect_inst(
        thumb_x - 0.5,
        thumb_y + 1.0,
        diameter + 1.0,
        diameter,
        IOS_THUMB_SHADOW,
        [diameter * 0.5; 4],
        [0.0; 4],
        0.0,
    ));
    rects.push(rect_inst(
        thumb_x,
        thumb_y,
        diameter,
        diameter,
        IOS_THUMB,
        [diameter * 0.5; 4],
        [0.0; 4],
        0.0,
    ));
}

// ---------------------------------------------------------------------------
// Slider — UISlider
// ---------------------------------------------------------------------------

/// Thin rounded track centered vertically. The `min..value` range
/// is iOS blue; `value..max` is translucent gray. Thumb is a 28pt
/// white circle with a soft drop shadow.
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
    let thumb_size = SLIDER_THUMB_SIZE;
    let track_h = SLIDER_TRACK_HEIGHT;
    let inset = thumb_size * 0.5;
    let track_x = x + inset;
    let track_y = y + (h - track_h) * 0.5;
    let track_w = (w - inset * 2.0).max(0.0);

    let t = if max > min {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let fill_w = track_w * t;

    rects.push(rect_inst(
        track_x,
        track_y,
        track_w,
        track_h,
        IOS_TRACK_OFF,
        [track_h * 0.5; 4],
        [0.0; 4],
        0.0,
    ));
    if fill_w > 0.0 {
        rects.push(rect_inst(
            track_x,
            track_y,
            fill_w,
            track_h,
            IOS_BLUE,
            [track_h * 0.5; 4],
            [0.0; 4],
            0.0,
        ));
    }

    let thumb_x = track_x + fill_w - thumb_size * 0.5;
    let thumb_y = y + (h - thumb_size) * 0.5;

    rects.push(rect_inst(
        thumb_x - 0.5,
        thumb_y + 1.5,
        thumb_size + 1.0,
        thumb_size,
        IOS_THUMB_SHADOW,
        [thumb_size * 0.5; 4],
        [0.0; 4],
        0.0,
    ));
    rects.push(rect_inst(
        thumb_x,
        thumb_y,
        thumb_size,
        thumb_size,
        IOS_THUMB,
        [thumb_size * 0.5; 4],
        [0.0; 4],
        0.0,
    ));
}

// ---------------------------------------------------------------------------
// TextInput — UITextField
// ---------------------------------------------------------------------------

/// Rounded white field with a hairline gray border (or iOS-blue
/// focused border). Caret is a 1.5pt vertical iOS-blue rect at
/// the end of the value text, painted only on the on-phase of
/// the caret blink. Placeholder text gets a muted gray.
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
    let border = if is_focused { IOS_BLUE } else { IOS_TEXT_BORDER };
    rects.push(rect_inst(
        x,
        y,
        w,
        h,
        IOS_TEXT_INPUT_BG,
        [8.0; 4],
        border,
        1.0,
    ));

    let glyph_color = if is_placeholder { IOS_PLACEHOLDER } else { text_color };
    let text_x = x + TEXT_INPUT_HPAD;
    let text_y = y + TEXT_INPUT_VPAD;
    let inner_w = (w - TEXT_INPUT_HPAD * 2.0).max(0.0);
    texts.push(StagedText {
        buffer,
        x: text_x,
        y: text_y,
        color: glyph_color,
        clip: TextBounds {
            left: text_x as i32,
            top: y as i32,
            right: (text_x + inner_w) as i32,
            bottom: (y + h) as i32,
        },
    });

    if is_focused && draw_caret && !is_placeholder {
        let caret_x = text_x + caret_x_local;
        let caret_y = y + TEXT_INPUT_VPAD;
        let caret_h = h - TEXT_INPUT_VPAD * 2.0;
        rects.push(rect_inst(
            caret_x,
            caret_y,
            TEXT_INPUT_CARET_WIDTH,
            caret_h,
            IOS_BLUE,
            [0.0; 4],
            [0.0; 4],
            0.0,
        ));
    }
}
