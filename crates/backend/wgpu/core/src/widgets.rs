//! Native-widget rendering for `Toggle`, `Slider`, and `TextInput`.
//!
//! The framework's primitive tree is the same across simulated
//! platforms — what differs is the *look* the backend gives to
//! native controls. This module houses one paint function per
//! widget, each dispatching on [`SimulatedPlatform`].
//!
//! Currently only the iOS skin is fully implemented. Android paths
//! exist as `TODO`s that fall through to iOS so apps stay
//! renderable while Material 3 styling is being added.
//!
//! The widget paint functions all return through `rects` (for
//! background / track / thumb / caret) and `texts` (for input
//! value + placeholder). They never call wgpu directly — that's
//! the caller's job in `app::render_frame`.

use crate::animation::lerp_color;
use crate::app::SimulatedPlatform;
use crate::node::{
    SLIDER_THUMB_SIZE, SLIDER_TRACK_HEIGHT, TEXT_INPUT_CARET_WIDTH, TOGGLE_THUMB_INSET,
};
use crate::pipeline::Instance as RectInstance;
use crate::style_convert::srgb_rgba_to_linear;
use crate::text::StagedText;
use glyphon::{Buffer, TextBounds};

// ---------------------------------------------------------------------------
// iOS palette (matches UIKit's `systemBlue`, `systemGreen`, the
// translucent `systemGray5`-style track-off, etc.)
// ---------------------------------------------------------------------------

const IOS_BLUE: [f32; 4] = [0.0, 0x7a as f32 / 255.0, 1.0, 1.0];          // #007AFF
const IOS_GREEN: [f32; 4] = [0x34 as f32 / 255.0, 0xC7 as f32 / 255.0, 0x59 as f32 / 255.0, 1.0]; // #34C759
const IOS_TRACK_OFF: [f32; 4] = [0x78 as f32 / 255.0, 0x78 as f32 / 255.0, 0x80 as f32 / 255.0, 0.32]; // systemGray with alpha
const IOS_THUMB: [f32; 4] = [1.0, 1.0, 1.0, 1.0];                        // white
const IOS_THUMB_SHADOW: [f32; 4] = [0.0, 0.0, 0.0, 0.12];                // soft drop under thumb
const IOS_TEXT_BORDER: [f32; 4] = [0xC7 as f32 / 255.0, 0xC7 as f32 / 255.0, 0xCC as f32 / 255.0, 1.0]; // systemGray3
const IOS_TEXT_INPUT_BG: [f32; 4] = [1.0, 1.0, 1.0, 1.0];                // white field
const IOS_PLACEHOLDER: [f32; 4] = [60.0 / 255.0, 60.0 / 255.0, 67.0 / 255.0, 0.6];

// ---------------------------------------------------------------------------
// Toggle
// ---------------------------------------------------------------------------

/// Append rect instances for a Toggle at the given frame.
///
/// `t` is the continuous thumb position in `0..=1`:
/// - `0.0` = OFF (thumb at left, track in gray)
/// - `1.0` = ON (thumb at right, track in iOS green)
/// - any value in between = mid-animation
///
/// iOS look: 51×31 pill with white circular thumb. Track color
/// blends from gray to green as `t` increases; thumb position
/// linearly interpolates between the two ends.
pub fn paint_toggle(
    platform: SimulatedPlatform,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    t: f32,
    rects: &mut Vec<RectInstance>,
) {
    match platform {
        SimulatedPlatform::Ios | SimulatedPlatform::Android => {
            // TODO(android): once Material 3 styling exists, branch
            // here. iOS is the reference look in the meantime.
            paint_toggle_ios(x, y, w, h, t, rects);
        }
    }
}

fn paint_toggle_ios(x: f32, y: f32, w: f32, h: f32, t: f32, rects: &mut Vec<RectInstance>) {
    let t = t.clamp(0.0, 1.0);
    let track_radius = h * 0.5;
    // Smoothly blend the track color across the animation.
    let track_color = lerp_color(IOS_TRACK_OFF, IOS_GREEN, t);

    // Track.
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

    // Thumb: a circular white knob inset from the track by
    // `TOGGLE_THUMB_INSET`. Diameter = h - 2*inset. Linearly
    // interpolate its left edge between the OFF anchor and the
    // ON anchor.
    let diameter = h - TOGGLE_THUMB_INSET * 2.0;
    let thumb_x_off = x + TOGGLE_THUMB_INSET;
    let thumb_x_on = x + w - TOGGLE_THUMB_INSET - diameter;
    let thumb_x = thumb_x_off + (thumb_x_on - thumb_x_off) * t;
    let thumb_y = y + TOGGLE_THUMB_INSET;

    // Drop shadow under the thumb — 1px down, slightly wider.
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
    // The knob.
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
// Slider
// ---------------------------------------------------------------------------

/// Append rect instances for a Slider at the given frame.
///
/// iOS look: thin rounded track centered vertically. The portion
/// from `min..value` is filled in iOS blue; the portion from
/// `value..max` is gray. The thumb is a 28px white circle with a
/// soft shadow.
pub fn paint_slider(
    platform: SimulatedPlatform,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    value: f32,
    min: f32,
    max: f32,
    rects: &mut Vec<RectInstance>,
) {
    match platform {
        SimulatedPlatform::Ios | SimulatedPlatform::Android => {
            paint_slider_ios(x, y, w, h, value, min, max, rects);
        }
    }
}

fn paint_slider_ios(
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
    // Inset the track horizontally by the thumb radius so the
    // thumb stays inside the rect bounds at both extremes.
    let inset = thumb_size * 0.5;
    let track_x = x + inset;
    let track_y = y + (h - track_h) * 0.5;
    let track_w = (w - inset * 2.0).max(0.0);

    // Normalized progress in [0..1]; clamp so out-of-range values
    // don't draw past the track ends.
    let t = if max > min {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let fill_w = track_w * t;

    // Empty (right) portion.
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
    // Filled (left) portion — drawn on top so the right edge meets
    // the thumb cleanly.
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

    // Thumb position centered on the fill end.
    let thumb_x = track_x + fill_w - thumb_size * 0.5;
    let thumb_y = y + (h - thumb_size) * 0.5;

    // Drop shadow.
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
    // The thumb itself.
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
// TextInput
// ---------------------------------------------------------------------------

/// Horizontal padding inside the text-input box. Matches UIKit's
/// `UITextField` default content inset.
pub const TEXT_INPUT_HPAD: f32 = 10.0;
/// Vertical padding inside the text-input box. Centers a 17pt
/// system font in a 36pt field.
pub const TEXT_INPUT_VPAD: f32 = 8.0;

/// Append rect + text instances for a TextInput.
///
/// `text_color` is the resolved text color from the node's style
/// (RenderStyle.color). `placeholder_buffer` and `value_buffer`
/// are the glyphon buffers prepared by the backend's text store;
/// `caret_x_local` is the cursor's pixel offset from the left of
/// the value text (in logical px), used to position the caret rect.
#[allow(clippy::too_many_arguments)]
pub fn paint_text_input<'a>(
    platform: SimulatedPlatform,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    is_focused: bool,
    is_placeholder: bool,
    buffer: &'a Buffer,
    caret_x_local: f32,
    text_color: [f32; 4],
    rects: &mut Vec<RectInstance>,
    texts: &mut Vec<StagedText<'a>>,
) {
    match platform {
        SimulatedPlatform::Ios | SimulatedPlatform::Android => {
            paint_text_input_ios(
                x,
                y,
                w,
                h,
                is_focused,
                is_placeholder,
                buffer,
                caret_x_local,
                text_color,
                rects,
                texts,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_text_input_ios<'a>(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    is_focused: bool,
    is_placeholder: bool,
    buffer: &'a Buffer,
    caret_x_local: f32,
    text_color: [f32; 4],
    rects: &mut Vec<RectInstance>,
    texts: &mut Vec<StagedText<'a>>,
) {
    // Field background + border. Focused fields show an iOS-blue
    // border; unfocused use the systemGray3 hairline.
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

    // Text glyphs: placeholder gets a muted color; value gets the
    // node's resolved style color.
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

    // Caret — drawn only when focused and we're showing the real
    // value (not placeholder). No blinking in the MVP; we'd hook a
    // time-driven Effect for that.
    if is_focused && !is_placeholder {
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `RectInstance` with sRGB→linear color conversion baked
/// in (the surface is sRGB-encoded; see `style_convert`).
fn rect_inst(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    bg_srgb: [f32; 4],
    corner_radius: [f32; 4],
    border_color_srgb: [f32; 4],
    border_width: f32,
) -> RectInstance {
    RectInstance {
        rect: [x, y, w, h],
        bg: srgb_rgba_to_linear(bg_srgb),
        corner_radius,
        border_color: srgb_rgba_to_linear(border_color_srgb),
        border_width,
        _pad: [0.0; 3],
    }
}
