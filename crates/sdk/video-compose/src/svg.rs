//! Rasterize an SVG asset to an RGBA [`ImageSource`] so it can flow through the
//! normal watermark-image path (corner placement, reactive opacity) on every
//! backend — including web. Uses `resvg`/`usvg`/`tiny-skia` (pure Rust, wasm-safe;
//! `resvg` re-exports both). Gated behind the `svg` feature since resvg is heavy.
//!
//! The renderer is built without text/system-font support (`default-features =
//! false`), so text and embedded raster images inside the SVG are dropped —
//! convert text to paths first. Vector logos rasterize faithfully.

use canvas_core::ImageSource;
use resvg::{tiny_skia, usvg};
use std::hash::{Hash, Hasher};

/// Render `bytes` (an SVG document) at `width_px` wide (height derived from the
/// SVG's aspect ratio) into a straight-alpha RGBA [`ImageSource`]. `None` if the
/// SVG can't be parsed or has no area. The image `id` is a hash of
/// `(bytes, width_px)` so re-rendering the same asset reuses the upload cache.
pub(crate) fn rasterize(bytes: &[u8], width_px: u32) -> Option<ImageSource> {
    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).ok()?;
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return None;
    }
    let out_w = width_px.max(1);
    let scale = out_w as f32 / size.width();
    let out_h = (size.height() * scale).round().max(1.0) as u32;

    let mut pixmap = tiny_skia::Pixmap::new(out_w, out_h)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    // tiny-skia stores PREMULTIPLIED alpha; the layer shader (and `ImageSource`)
    // expect STRAIGHT alpha — un-premultiply so semi-transparent edges tint right.
    let mut rgba = pixmap.data().to_vec();
    for px in rgba.chunks_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            px[0] = (px[0] as u32 * 255 / a).min(255) as u8;
            px[1] = (px[1] as u32 * 255 / a).min(255) as u8;
            px[2] = (px[2] as u32 * 255 / a).min(255) as u8;
        }
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    out_w.hash(&mut hasher);
    Some(ImageSource::from_rgba8(hasher.finish(), out_w, out_h, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal SVG: a 50%-opacity red 100×60 rect (tests parse + un-premultiply).
    const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="60">
        <rect width="100" height="60" fill="rgb(255,0,0)" fill-opacity="0.5"/></svg>"#;

    #[test]
    fn rasterizes_svg_at_requested_width_keeping_aspect() {
        let img = rasterize(SVG, 200).expect("rasterized");
        assert!(img.is_valid());
        assert_eq!(img.width, 200);
        assert_eq!(img.height, 120, "height follows the 100:60 aspect");
        // Center pixel: straight-alpha red at ~50% (un-premultiplied, so r≈255).
        let i = (((img.height / 2) * img.width + img.width / 2) * 4) as usize;
        assert!(img.rgba[i] > 200 && img.rgba[i + 1] < 60, "red, got {:?}", &img.rgba[i..i + 4]);
        assert!((100..=160).contains(&(img.rgba[i + 3] as u32)), "~50% alpha, got {}", img.rgba[i + 3]);
    }

    #[test]
    fn unparseable_svg_is_none() {
        assert!(rasterize(b"not an svg", 100).is_none());
    }
}
