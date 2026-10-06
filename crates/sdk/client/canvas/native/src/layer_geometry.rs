//! Pure texture-layer composite math for the Android renderer: how the cropped
//! source sub-rect maps onto the drawn rect, and the border frame's color.
//!
//! No platform dependency, so `tests/layer_geometry.rs` includes this file by
//! path and checks it on every host, while `android.rs` (which only compiles
//! for Android) includes it by `#[path]` the same way it includes `pixels.rs`.
//! What stays untested off-device is only the JNI plumbing that hands these
//! numbers to `Matrix` / `Paint`, plus the `clipRect` call (see the README's
//! Android checklist).

#![allow(dead_code)] // each including module uses a subset

/// The scale + translate that maps the source sub-rect `src` (bitmap pixels,
/// fractional) exactly onto `dst` (canvas units), as
/// `(scale_x, scale_y, translate_x, translate_y)`: a source point `p` lands at
/// `p * scale + translate`.
///
/// It is applied as `Matrix.setScale(scale_x, scale_y)` followed by
/// `Matrix.postTranslate(translate_x, translate_y)` and drawn with
/// `drawBitmap(Bitmap, Matrix, Paint)`. The `drawBitmap(Rect, RectF)` overload
/// that Android code usually reaches for takes an INTEGER source rect, so a
/// crop or a Cover offset that starts mid-pixel snapped to whole source pixels
/// and the framing drifted from web / Apple / vello, which sample the exact
/// `TextureLayer::source_rects` sub-rect.
///
/// The matrix draws the WHOLE bitmap, so the caller must clip to `dst` — the
/// pixels outside the sub-rect are mapped outside `dst`, not dropped.
///
/// Returns `None` for an empty source rect (nothing to map).
pub fn source_to_dest(
    src: (f32, f32, f32, f32),
    dst: (f32, f32, f32, f32),
) -> Option<(f32, f32, f32, f32)> {
    let (sx, sy, sw, sh) = src;
    let (dx, dy, dw, dh) = dst;
    if sw <= 0.0 || sh <= 0.0 {
        return None;
    }
    let (kx, ky) = (dw / sw, dh / sh);
    Some((kx, ky, dx - sx * kx, dy - sy * ky))
}

/// The border frame's `0xAARRGGBB` color for `android.graphics.Paint.setColor`:
/// the border color with its alpha scaled by the layer `opacity` (clamped to
/// `0..=1`), so the frame fades with the picture — as on web, where
/// `globalAlpha` covers both, and in vello (`border alpha × opacity`).
pub fn border_argb(r: u8, g: u8, b: u8, a: u8, opacity: f32) -> i32 {
    let a = (a as f32 * opacity.clamp(0.0, 1.0)).round() as u32;
    ((a << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32) as i32
}
