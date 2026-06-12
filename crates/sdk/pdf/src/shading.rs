//! Render a PDF **shading pattern** (axial, radial, function-based, or mesh) to
//! an RGBA texture, clipped to the fill path.
//!
//! PDF gradients/shadings don't map cleanly onto a fixed-stop canvas gradient
//! (the color is an arbitrary function, the coordinate mapping carries the
//! pattern matrix, and there are 7 shading types). `hayro` already solves all
//! that: [`ShadingPattern::encode`] produces an [`EncodedShadingPattern`] whose
//! [`sample`](hayro_interpret::encode::EncodedShadingPattern::sample) gives the
//! RGBA color at any point (in the encoded pattern's space, reached by applying
//! its `base_transform` to a device point). So we sample the shading per texel
//! over the fill's device-space bounding box — exactly the reference renderer's
//! approach — and hand the result to the canvas as an image, clipped to the
//! path. Correct for every shading type, at the cost of being raster (a smooth
//! gradient upscales cleanly, so this is rarely visible).

use canvas_core::{ImageSource, Rect};
use hayro_interpret::pattern::ShadingPattern;
use hayro_interpret::CacheKey;
use kurbo::{BezPath, Point, Shape};

/// Cap on a shading texture's larger dimension — a full-page gradient is sampled
/// at most this many texels per side (it's smooth, so downscaling is invisible).
const MAX_SHADING_DIM: u32 = 1024;

/// Render `shading` over `device_path`'s bounding box into a straight-RGBA8
/// texture. `device_path` is the fill path already mapped to device/logical
/// space (the CTM applied). Returns the texture + its destination rect in that
/// same space, or `None` for a degenerate box.
pub(crate) fn render(
    shading: &ShadingPattern,
    device_path: &BezPath,
) -> Option<(ImageSource, Rect)> {
    let bbox = device_path.bounding_box();
    if bbox.width() < 0.5 || bbox.height() < 0.5 {
        return None;
    }
    let encoded = shading.encode();

    let (w, h) = fit(bbox.width(), bbox.height(), MAX_SHADING_DIM);
    // device units per texel (1.0 unless the box exceeds the cap).
    let (sx, sy) = (bbox.width() / w as f64, bbox.height() / h as f64);

    let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for row in 0..h {
        for col in 0..w {
            let device = Point::new(
                bbox.x0 + (col as f64 + 0.5) * sx,
                bbox.y0 + (row as f64 + 0.5) * sy,
            );
            // base_transform maps a device point into the shading's space, where
            // `sample` evaluates the (function-driven) color. Straight sRGB RGBA.
            let s = encoded.sample(encoded.base_transform * device);
            rgba.push(to_u8(s[0]));
            rgba.push(to_u8(s[1]));
            rgba.push(to_u8(s[2]));
            rgba.push(to_u8(s[3]));
        }
    }

    let src = ImageSource::from_rgba8(shading.cache_key() as u64, w, h, rgba);
    let dst = Rect::new(bbox.x0 as f32, bbox.y0 as f32, bbox.width() as f32, bbox.height() as f32);
    Some((src, dst))
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Texture dimensions for a `w × h` device box, capped to `max` on the larger
/// side (aspect preserved).
fn fit(w: f64, h: f64, max: u32) -> (u32, u32) {
    let (w, h) = (w.ceil().max(1.0), h.ceil().max(1.0));
    let m = w.max(h);
    if m <= max as f64 {
        return (w as u32, h as u32);
    }
    let s = max as f64 / m;
    ((w * s).round().max(1.0) as u32, (h * s).round().max(1.0) as u32)
}
