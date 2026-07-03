//! Rasterize a text string with a caller-supplied font into an RGBA
//! [`ImageSource`], so it can flow through the normal watermark-image path
//! (corner placement, reactive opacity) on EVERY backend — including web, where
//! the `.draw()` glyph path isn't rendered.
//!
//! Uses `cosmic-text` (the shaper the GPU backend already uses via `glyphon`)
//! with an explicit font database seeded from the caller's bytes: no system-font
//! scan, so it's deterministic and works on wasm (which has no system fonts).

use canvas_core::{Color, ImageSource};
use cosmic_text::{
    fontdb::Database, Attrs, Buffer, Color as CosmicColor, Family, FontSystem, Metrics, Shaping,
    SwashCache,
};
use std::hash::{Hash, Hasher};

/// Shape `text` in `font` at `size_px` and rasterize it, tinted to `color`
/// (straight alpha = glyph coverage × `color.a`), into a tightly-cropped RGBA
/// [`ImageSource`]. `None` if the font can't be parsed or the text is empty.
///
/// The image `id` is a hash of `(text, size, color)` so the renderer's upload
/// cache reuses it across frames and two different text watermarks don't collide.
pub(crate) fn rasterize(text: &str, font: &[u8], size_px: f32, color: Color) -> Option<ImageSource> {
    if text.is_empty() || size_px <= 0.0 {
        return None;
    }

    // A db seeded with ONLY the caller's font — no system scan (wasm-safe).
    let mut db = Database::new();
    db.load_font_data(font.to_vec());
    let family = db.faces().next()?.families.first().map(|(n, _)| n.clone());
    let mut fs = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
    let mut cache = SwashCache::new();

    let attrs = Attrs::new();
    let attrs = match &family {
        Some(name) => attrs.family(Family::Name(name)),
        None => attrs,
    };

    let mut buffer = Buffer::new(&mut fs, Metrics::new(size_px, size_px * 1.25));
    // Unbounded: a single line grows to the text's natural width.
    buffer.set_size(&mut fs, None, None);
    buffer.set_text(&mut fs, text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(&mut fs, false);

    // Measure the shaped text's bounding box.
    let mut w = 0f32;
    let mut h = 0f32;
    for run in buffer.layout_runs() {
        w = w.max(run.line_w);
        h = h.max(run.line_top + run.line_height);
    }
    if w <= 0.0 || h <= 0.0 {
        return None;
    }

    // Pad to catch glyph overshoot (side bearings, descenders) beyond the box.
    let pad = (size_px * 0.25).ceil() as i32;
    let img_w = w.ceil() as i32 + 2 * pad;
    let img_h = h.ceil() as i32 + 2 * pad;
    if img_w <= 0 || img_h <= 0 {
        return None;
    }
    let (img_w, img_h) = (img_w as u32, img_h as u32);
    let mut rgba = vec![0u8; (img_w * img_h * 4) as usize];

    // Draw glyph pixels; `c.a()` is the coverage (0..255) of this pixel already
    // tinted to the text color. Scale by the requested `color.a` for translucency.
    let text_color = CosmicColor::rgb(color.r, color.g, color.b);
    buffer.draw(&mut fs, &mut cache, text_color, |x, y, gw, gh, c| {
        let cov = c.a() as u32;
        if cov == 0 {
            return;
        }
        for dy in 0..gh as i32 {
            for dx in 0..gw as i32 {
                let px = x + dx + pad;
                let py = y + dy + pad;
                if px < 0 || py < 0 || px as u32 >= img_w || py as u32 >= img_h {
                    continue;
                }
                let idx = ((py as u32 * img_w + px as u32) * 4) as usize;
                let a = (cov * color.a as u32 / 255) as u8;
                rgba[idx] = c.r();
                rgba[idx + 1] = c.g();
                rgba[idx + 2] = c.b();
                rgba[idx + 3] = a;
            }
        }
    });

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    size_px.to_bits().hash(&mut hasher);
    [color.r, color.g, color.b, color.a].hash(&mut hasher);
    let id = hasher.finish();

    Some(ImageSource::from_rgba8(id, img_w, img_h, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real font so the shaper has glyphs to lay out (Inter, already in the repo).
    const FONT: &[u8] = include_bytes!("../../../../examples/welcome/fonts/Inter-Regular.ttf");

    #[test]
    fn rasterizes_text_to_a_nonempty_image() {
        let img = rasterize("Hello", FONT, 32.0, Color::new(255, 255, 255, 255)).expect("rasterized");
        assert!(img.is_valid());
        assert!(img.width > 20 && img.height > 10, "sized to the text, got {}x{}", img.width, img.height);
        // The glyphs must produce visible (non-transparent) pixels.
        assert!(img.rgba.chunks(4).any(|p| p[3] > 40), "text should ink some pixels");
    }

    #[test]
    fn empty_text_or_unparseable_font_is_none() {
        assert!(rasterize("", FONT, 32.0, Color::new(255, 255, 255, 255)).is_none());
        assert!(rasterize("hi", &[1, 2, 3], 32.0, Color::new(255, 255, 255, 255)).is_none());
        assert!(rasterize("hi", FONT, 0.0, Color::new(255, 255, 255, 255)).is_none());
    }

    #[test]
    fn color_tint_and_alpha_are_applied() {
        // Red text at 50% alpha: inked pixels are red, and their alpha is scaled
        // down by color.a (glyph coverage × 128/255 ≤ ~128).
        let img = rasterize("A", FONT, 48.0, Color::new(255, 0, 0, 128)).unwrap();
        let inked = img.rgba.chunks(4).find(|p| p[3] > 20).expect("some ink");
        assert!(inked[0] > 200 && inked[1] < 60 && inked[2] < 60, "tinted red, got {inked:?}");
        assert!(inked[3] <= 130, "alpha scaled by color.a, got {}", inked[3]);
    }

    #[test]
    fn same_text_hashes_to_same_id() {
        let a = rasterize("v1.0", FONT, 24.0, Color::new(0, 0, 0, 255)).unwrap();
        let b = rasterize("v1.0", FONT, 24.0, Color::new(0, 0, 0, 255)).unwrap();
        let c = rasterize("v2.0", FONT, 24.0, Color::new(0, 0, 0, 255)).unwrap();
        assert_eq!(a.id, b.id, "identical text → stable cache id");
        assert_ne!(a.id, c.id, "different text → different id");
    }
}
