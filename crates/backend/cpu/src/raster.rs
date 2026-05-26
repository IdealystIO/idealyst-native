//! Software rasterizer primitives. Backend-internal — surfaces only
//! see the final `put_pixel` / `fill_rect` calls.
//!
//! Everything here is axis-aligned. Rotation / shears would force a
//! per-pixel inverse transform that's prohibitively expensive on the
//! ESP32-class chips this backend ultimately targets, so the trade
//! is: arbitrary affine transforms are NOT supported, but axis-
//! aligned rounded rectangles and bitmap glyphs paint at near
//! memcpy speed.
//!
//! ## Alpha blending
//!
//! All draw operations take an `[r, g, b, a]` source color. The
//! rasterizer reads the surface to be blendable — but reading from
//! a `Surface` is *not* on the trait (surfaces may be write-only,
//! e.g. an SPI display). Instead, the caller provides the
//! destination color through whatever scheme the backend uses to
//! track "what's currently painted at (x, y)" — for the MVP the
//! `MemSurface` exposes `get_pixel`, but the higher-level paint
//! walker only emits blended pixels for the *background* layer of
//! each node (computing dst by reading the framebuffer). For
//! write-only surfaces this means alpha-on-alpha doesn't accumulate
//! correctly — that's a known limitation flagged in the README.

use crate::surface::Surface;

/// Source-over composite of `src` over `dst`, both in straight (non-
/// premultiplied) sRGB. Returns an opaque color (alpha = 255). This
/// is the workhorse the paint walker uses any time it draws a
/// translucent color onto an already-painted background.
///
/// We blend in 8-bit gamma space (NOT linear). That's wrong by the
/// physics of how light works, but matches every other backend in
/// this repo (web/CSS, UIKit, Android) and avoids a per-pixel
/// `pow(srgb, 2.2)` round-trip — which on ESP32-class targets would
/// halve our fill rate.
#[inline]
pub(crate) fn blend_over(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    let sa = src[3] as u32;
    if sa == 255 {
        return [src[0], src[1], src[2], 255];
    }
    if sa == 0 {
        return [dst[0], dst[1], dst[2], 255];
    }
    let inv = 255 - sa;
    // Each channel: (src * sa + dst * (255 - sa)) / 255, with
    // round-to-nearest via the `+ 127` trick.
    let mix = |s: u8, d: u8| -> u8 {
        let v = (s as u32) * sa + (d as u32) * inv;
        ((v + 127) / 255) as u8
    };
    [mix(src[0], dst[0]), mix(src[1], dst[1]), mix(src[2], dst[2]), 255]
}

/// Multiply a source alpha by the parent's effective opacity (in
/// `[0, 1]`). Convenience because the paint walker carries opacity
/// as `f32` and needs to fold it into the 8-bit alpha channel.
#[inline]
pub fn premultiply_alpha(color: [u8; 4], opacity: f32) -> [u8; 4] {
    if opacity >= 0.999 {
        return color;
    }
    if opacity <= 0.001 {
        return [color[0], color[1], color[2], 0];
    }
    let a = (color[3] as f32 * opacity).round().clamp(0.0, 255.0) as u8;
    [color[0], color[1], color[2], a]
}

/// Fill an axis-aligned rectangle with `color`, blending against the
/// surface's existing pixels when `color`'s alpha is less than 255.
///
/// `clip` is the active clip rectangle in surface coordinates —
/// pixels outside it are skipped. The paint walker uses this to
/// implement scroll-view clipping.
pub fn fill_rect_blended<S: Surface>(
    surface: &mut S,
    rect: Rect,
    color: [u8; 4],
    clip: Rect,
    dst_sampler: impl Fn(&S, u32, u32) -> [u8; 4],
) {
    let drawn = match rect.intersect(clip) {
        Some(r) => r,
        None => return,
    };
    let sw = surface.width() as i32;
    let sh = surface.height() as i32;
    let x0 = drawn.x.max(0);
    let y0 = drawn.y.max(0);
    let x1 = (drawn.x + drawn.w as i32).min(sw);
    let y1 = (drawn.y + drawn.h as i32).min(sh);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    if color[3] == 255 {
        // Opaque fast path — let the surface use its (potentially
        // SIMD / DMA-driven) `fill_rect` implementation.
        surface.fill_rect(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32, color);
        return;
    }
    // Blend path: per-pixel composite.
    for py in y0..y1 {
        for px in x0..x1 {
            let dst = dst_sampler(surface, px as u32, py as u32);
            let out = blend_over(color, dst);
            surface.put_pixel(px as u32, py as u32, out);
        }
    }
}

/// Fill a rounded rectangle. Corners may have different radii (CSS
/// `border-top-left-radius` etc.). Uses per-pixel circle inclusion
/// inside the corner squares and treats the rest as a regular fill.
///
/// `radii` is `[tl, tr, br, bl]` in pixels. Radii are clamped so the
/// sum on any side doesn't exceed that side's length (mirrors CSS
/// behavior + matches the iOS clamp described in
/// `project_ios_cornerradius_unclamped`).
pub fn fill_rounded_rect_blended<S: Surface>(
    surface: &mut S,
    rect: Rect,
    radii: [f32; 4],
    color: [u8; 4],
    clip: Rect,
    dst_sampler: impl Fn(&S, u32, u32) -> [u8; 4],
) {
    let radii = clamp_radii(radii, rect.w as f32, rect.h as f32);
    let [tl, tr, br, bl] = radii;
    // Fast path: no rounded corners → rectangle fill.
    if tl <= 0.5 && tr <= 0.5 && br <= 0.5 && bl <= 0.5 {
        fill_rect_blended(surface, rect, color, clip, dst_sampler);
        return;
    }
    let drawn = match rect.intersect(clip) {
        Some(r) => r,
        None => return,
    };
    let sw = surface.width() as i32;
    let sh = surface.height() as i32;
    let x0 = drawn.x.max(0);
    let y0 = drawn.y.max(0);
    let x1 = (drawn.x + drawn.w as i32).min(sw);
    let y1 = (drawn.y + drawn.h as i32).min(sh);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    // Per-pixel inclusion test against the rounded shape. This is
    // O(W·H) per node — the natural cost of arbitrary corner radii.
    // ESP32-class targets should keep radii small (<= 6px) so the
    // hit count stays bounded.
    let rx0 = rect.x as f32;
    let ry0 = rect.y as f32;
    let rx1 = (rect.x + rect.w as i32) as f32;
    let ry1 = (rect.y + rect.h as i32) as f32;
    for py in y0..y1 {
        let cy = py as f32 + 0.5;
        for px in x0..x1 {
            let cx = px as f32 + 0.5;
            // Coverage in [0, 1] — we use 1-pixel anti-aliasing by
            // computing the distance from the rounded-shape boundary
            // and treating any pixel center inside as covered. AA on
            // corners would require multi-sampling; skip on this
            // hot path.
            let inside = rounded_rect_contains(cx, cy, rx0, ry0, rx1, ry1, tl, tr, br, bl);
            if !inside {
                continue;
            }
            if color[3] == 255 {
                surface.put_pixel(px as u32, py as u32, [color[0], color[1], color[2], 255]);
            } else {
                let dst = dst_sampler(surface, px as u32, py as u32);
                let out = blend_over(color, dst);
                surface.put_pixel(px as u32, py as u32, out);
            }
        }
    }
}

/// Paint a 1-axis-aligned border around `rect`. `widths` is
/// `[top, right, bottom, left]`; `colors` is `[top, right, bottom,
/// left]`. Zero-width or `None`-colored sides are skipped.
///
/// This is the simple "four rectangles" model — corners aren't
/// mitered, which means non-uniform border colors will show seams
/// at corners. Acceptable for the MVP (no consuming SDK uses mixed
/// border colors); uniform-border-color is correct.
pub fn stroke_border<S: Surface>(
    surface: &mut S,
    rect: Rect,
    widths: [f32; 4],
    colors: [Option<[u8; 4]>; 4],
    clip: Rect,
    dst_sampler: impl Fn(&S, u32, u32) -> [u8; 4],
) {
    let [wt, wr, wb, wl] = widths;
    let [ct, cr, cb, cl] = colors;
    if wt > 0.0 {
        if let Some(c) = ct {
            fill_rect_blended(
                surface,
                Rect::new(rect.x, rect.y, rect.w, wt.round() as u32),
                c,
                clip,
                &dst_sampler,
            );
        }
    }
    if wb > 0.0 {
        if let Some(c) = cb {
            let h = wb.round() as i32;
            fill_rect_blended(
                surface,
                Rect::new(rect.x, rect.y + rect.h as i32 - h, rect.w, h as u32),
                c,
                clip,
                &dst_sampler,
            );
        }
    }
    if wl > 0.0 {
        if let Some(c) = cl {
            fill_rect_blended(
                surface,
                Rect::new(rect.x, rect.y, wl.round() as u32, rect.h),
                c,
                clip,
                &dst_sampler,
            );
        }
    }
    if wr > 0.0 {
        if let Some(c) = cr {
            let w = wr.round() as i32;
            fill_rect_blended(
                surface,
                Rect::new(rect.x + rect.w as i32 - w, rect.y, w as u32, rect.h),
                c,
                clip,
                &dst_sampler,
            );
        }
    }
}

/// Public-to-the-crate alias for the private inclusion test —
/// callers in `lib.rs` (the gradient painter) use this to share the
/// same shape predicate the solid-fill path uses.
pub(crate) fn rounded_rect_contains_pub(
    cx: f32, cy: f32,
    x0: f32, y0: f32, x1: f32, y1: f32,
    tl: f32, tr: f32, br: f32, bl: f32,
) -> bool {
    rounded_rect_contains(cx, cy, x0, y0, x1, y1, tl, tr, br, bl)
}

/// Public-to-the-crate alias for the private radius clamp — same
/// rationale as `rounded_rect_contains_pub`.
pub(crate) fn clamp_radii_pub(radii: [f32; 4], w: f32, h: f32) -> [f32; 4] {
    clamp_radii(radii, w, h)
}

/// Per-pixel inclusion test for a rounded rectangle. Returns `true`
/// when the sample point lies inside the shape (rectangular interior
/// minus the four corner cutouts).
fn rounded_rect_contains(
    cx: f32, cy: f32,
    x0: f32, y0: f32, x1: f32, y1: f32,
    tl: f32, tr: f32, br: f32, bl: f32,
) -> bool {
    // Bounding-box reject first — cheaper than the per-corner
    // distance test when the sample is well inside.
    if cx < x0 || cx >= x1 || cy < y0 || cy >= y1 {
        return false;
    }
    // Top-left corner.
    if cx < x0 + tl && cy < y0 + tl {
        let dx = (x0 + tl) - cx;
        let dy = (y0 + tl) - cy;
        return dx * dx + dy * dy <= tl * tl;
    }
    // Top-right.
    if cx >= x1 - tr && cy < y0 + tr {
        let dx = cx - (x1 - tr);
        let dy = (y0 + tr) - cy;
        return dx * dx + dy * dy <= tr * tr;
    }
    // Bottom-right.
    if cx >= x1 - br && cy >= y1 - br {
        let dx = cx - (x1 - br);
        let dy = cy - (y1 - br);
        return dx * dx + dy * dy <= br * br;
    }
    // Bottom-left.
    if cx < x0 + bl && cy >= y1 - bl {
        let dx = (x0 + bl) - cx;
        let dy = cy - (y1 - bl);
        return dx * dx + dy * dy <= bl * bl;
    }
    // Interior — not in any corner cutout.
    true
}

/// Clamp corner radii so opposing radii don't sum to more than the
/// side length. Same rule iOS uses in
/// `project_ios_cornerradius_unclamped`: without this an over-large
/// radius produces a degenerate shape (nothing renders, or the AA
/// hits the wrong corner first).
fn clamp_radii(radii: [f32; 4], w: f32, h: f32) -> [f32; 4] {
    let [mut tl, mut tr, mut br, mut bl] = radii;
    let max_h = w * 0.5;
    let max_v = h * 0.5;
    let cap = max_h.min(max_v).max(0.0);
    tl = tl.max(0.0).min(cap);
    tr = tr.max(0.0).min(cap);
    br = br.max(0.0).min(cap);
    bl = bl.max(0.0).min(cap);
    [tl, tr, br, bl]
}

// ---------------------------------------------------------------------------
// Rect helper
// ---------------------------------------------------------------------------

/// Axis-aligned integer rectangle in surface pixel space.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// Surface-spanning clip — useful as the root clip for paint
    /// walks.
    pub const fn surface(width: u32, height: u32) -> Self {
        Self { x: 0, y: 0, w: width, h: height }
    }

    /// Returns the intersection of `self` and `other`, or `None` if
    /// they don't overlap.
    pub fn intersect(self, other: Rect) -> Option<Rect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w as i32).min(other.x + other.w as i32);
        let y1 = (self.y + self.h as i32).min(other.y + other.h as i32);
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        Some(Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_over_opaque_src_replaces_dst() {
        assert_eq!(blend_over([10, 20, 30, 255], [200, 100, 50, 255]), [10, 20, 30, 255]);
    }

    #[test]
    fn blend_over_zero_src_keeps_dst() {
        assert_eq!(blend_over([10, 20, 30, 0], [200, 100, 50, 255]), [200, 100, 50, 255]);
    }

    #[test]
    fn blend_over_half_alpha_averages() {
        // src = (200, 0, 0, 128), dst = (0, 200, 0, 255)
        // expected ≈ (100, 100, 0, 255) (within 1 of rounding)
        let out = blend_over([200, 0, 0, 128], [0, 200, 0, 255]);
        assert!((out[0] as i32 - 100).abs() <= 1, "got {:?}", out);
        assert!((out[1] as i32 - 100).abs() <= 1, "got {:?}", out);
        assert_eq!(out[2], 0);
        assert_eq!(out[3], 255);
    }

    #[test]
    fn rect_intersect_no_overlap_is_none() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(20, 20, 5, 5);
        assert_eq!(a.intersect(b), None);
    }

    #[test]
    fn rect_intersect_partial_overlap() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(b), Some(Rect::new(5, 5, 5, 5)));
    }

    #[test]
    fn clamp_radii_caps_at_half_min_side() {
        // 10x4 rect: max radius is 2 (h/2).
        let r = clamp_radii([10.0, 10.0, 10.0, 10.0], 10.0, 4.0);
        assert_eq!(r, [2.0, 2.0, 2.0, 2.0]);
    }
}
