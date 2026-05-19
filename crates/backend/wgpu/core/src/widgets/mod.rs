//! Native-widget rendering for `Toggle`, `Slider`, and
//! `TextInput`. Public surface is platform-agnostic — each
//! `paint_*` function dispatches to the matching
//! [`SimulatedPlatform`] module:
//!
//! - [`ios`] — Apple UIKit-style skin (UISwitch / UISlider /
//!   UITextField).
//! - [`android`] — Material 3-style skin.
//!
//! The two implementations are *parallel*. Both consume the
//! same renderer state (rect + text instance lists, glyph
//! buffer refs) and the same input rect from the layout pass.
//! Picking which one runs is a single `match` on the active
//! `SimulatedPlatform`; no shared per-widget state.
//!
//! # Shared infrastructure
//!
//! The [`rect_inst`] helper builds a `RectInstance` with the
//! sRGB→linear color conversion baked in. Both platform
//! modules use it so no paint code talks directly to the
//! `style_convert` module.

use backend_wgpu_api::SimulatedPlatform;
use glyphon::Buffer;

use crate::pipeline::Instance as RectInstance;
use crate::style_convert::srgb_rgba_to_linear;
use crate::text::StagedText;

pub mod android;
pub mod ios;

// ---------------------------------------------------------------------------
// Public surface — platform-agnostic paint dispatchers
// ---------------------------------------------------------------------------

/// Append rect instances for a Toggle at the given frame.
///
/// `t` is the continuous thumb position in `0..=1`:
/// - `0.0` = OFF (thumb at left, track in OFF color)
/// - `1.0` = ON (thumb at right, track in ON color)
/// - any value in between = mid-animation
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
        SimulatedPlatform::Ios => ios::paint_toggle(x, y, w, h, t, rects),
        SimulatedPlatform::Android => android::paint_toggle(x, y, w, h, t, rects),
    }
}

/// Append rect instances for a Slider at the given frame.
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
        SimulatedPlatform::Ios => ios::paint_slider(x, y, w, h, value, min, max, rects),
        SimulatedPlatform::Android => {
            android::paint_slider(x, y, w, h, value, min, max, rects)
        }
    }
}

/// Append rect + text instances for a TextInput.
#[allow(clippy::too_many_arguments)]
pub fn paint_text_input<'a>(
    platform: SimulatedPlatform,
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
    match platform {
        SimulatedPlatform::Ios => ios::paint_text_input(
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
        ),
        SimulatedPlatform::Android => android::paint_text_input(
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
        ),
    }
}

// ---------------------------------------------------------------------------
// Shared utility — used by both platform modules
// ---------------------------------------------------------------------------

/// Build a `RectInstance` with sRGB→linear color conversion baked
/// in (the surface is sRGB-encoded; see `style_convert`). The
/// platform modules build all their geometry through this helper
/// so the per-fragment color math stays in one place.
pub(crate) fn rect_inst(
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

// ---------------------------------------------------------------------------
// Shared layout constants for TextInput. (The widget-shape sizes
// — toggle 51×31, slider thumb 28 — live in `crate::node` since
// they're consulted by the Backend trait impl's intrinsic-size
// setup too.)
// ---------------------------------------------------------------------------

/// Horizontal padding inside the text-input box. Matches UIKit's
/// `UITextField` default content inset.
pub const TEXT_INPUT_HPAD: f32 = 10.0;
/// Vertical padding inside the text-input box. Centers a 17pt
/// system font in a 36pt field.
pub const TEXT_INPUT_VPAD: f32 = 8.0;
