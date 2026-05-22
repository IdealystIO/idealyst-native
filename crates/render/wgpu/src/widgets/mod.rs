//! Helpers + shared constants for native-widget paint code.
//!
//! Skin implementations (`ios-sim`, `android-sim`) draw all
//! widget chrome themselves; this module exposes only:
//!
//! - [`rect_inst`] / [`rect_inst_rotated`] — build a
//!   `RectInstance` with the sRGB→linear conversion baked in,
//!   so skin paint code never talks to `style_convert` directly.
//! - Inter-widget layout constants used by both the
//!   `Backend` impl's intrinsic-size setup and the skins.

use crate::pipeline::Instance as RectInstance;
use crate::style_convert::srgb_rgba_to_linear;

// ---------------------------------------------------------------------------
// Shared utility — used by every skin module
// ---------------------------------------------------------------------------

/// Build a `RectInstance` with sRGB→linear color conversion baked
/// in (the surface is sRGB-encoded; see `style_convert`). Skins
/// build all their geometry through this helper so the
/// per-fragment color math stays in one place.
pub fn rect_inst(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    bg_srgb: [f32; 4],
    corner_radius: [f32; 4],
    border_color_srgb: [f32; 4],
    border_width: f32,
) -> RectInstance {
    rect_inst_rotated(
        x,
        y,
        w,
        h,
        bg_srgb,
        corner_radius,
        border_color_srgb,
        border_width,
        0.0,
    )
}

/// Same as [`rect_inst`] but with a per-instance rotation around
/// the rect's center, in radians. Used by skins that need radial
/// geometry (the iOS spinner's capsule bars) while still going
/// through the central sRGB→linear conversion.
#[allow(clippy::too_many_arguments)]
pub fn rect_inst_rotated(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    bg_srgb: [f32; 4],
    corner_radius: [f32; 4],
    border_color_srgb: [f32; 4],
    border_width: f32,
    rotation: f32,
) -> RectInstance {
    RectInstance {
        rect: [x, y, w, h],
        bg: srgb_rgba_to_linear(bg_srgb),
        corner_radius,
        border_color: srgb_rgba_to_linear(border_color_srgb),
        border_width,
        rotation,
        shadow_blur: 0.0,
        ..bytemuck::Zeroable::zeroed()
    }
}

/// Build a *shadow* `RectInstance` — a rounded-rect drop shadow
/// rendered by the rect pipeline's shadow branch
/// (`shadow_blur > 0`). The quad covers the visual rect at
/// `(x, y, w, h)` shifted by `(offset_x, offset_y)` and expanded
/// by `blur` on every side; the shader fades the SDF's interior
/// at `bg.a * 1.0` to 0 across a `2*blur`-wide window. Pass the
/// *same* `corner_radius` as the rect you're shadowing so the
/// halo hugs its shape.
///
/// Push this *before* the main rect_inst so it paints behind.
#[allow(clippy::too_many_arguments)]
pub fn shadow_inst(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    offset_x: f32,
    offset_y: f32,
    blur: f32,
    color_srgb: [f32; 4],
    corner_radius: [f32; 4],
) -> RectInstance {
    let b = blur.max(0.0);
    RectInstance {
        rect: [x + offset_x - b, y + offset_y - b, w + b * 2.0, h + b * 2.0],
        bg: srgb_rgba_to_linear(color_srgb),
        corner_radius,
        border_color: [0.0; 4],
        border_width: 0.0,
        rotation: 0.0,
        shadow_blur: b,
        ..bytemuck::Zeroable::zeroed()
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
