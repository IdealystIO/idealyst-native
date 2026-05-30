//! PNG rasterization for an [`IconBlock`].
//!
//! The [`Source`] type wraps either:
//!
//! - a complete master image (SVG vector or decoded raster), or
//! - a foreground+background composite (gradient or solid backdrop
//!   with an SVG/raster glyph painted on top).
//!
//! Constructing one is the only entry point — see
//! [`Source::from_block`]. Once built it can be rendered at any size
//! via [`Source::render_png`] without re-decoding or re-parsing.

use anyhow::{anyhow, Context, Result};
use resvg::tiny_skia::{
    Color, GradientStop, LinearGradient, Paint, Pixmap, Point, Rect, SpreadMode, Transform,
};
use std::path::Path;

use crate::manifest::{Background, Gradient, IconBlock};

pub(crate) enum Source {
    /// `source` set on the block — rasterize as-is at every target.
    Standalone(StandaloneSource),
    /// `foreground` + `background` set — composite per render.
    Composite(CompositeSource),
}

pub(crate) enum StandaloneSource {
    Svg(resvg::usvg::Tree),
    Raster(image::DynamicImage),
}

pub(crate) struct CompositeSource {
    foreground: StandaloneSource,
    background: Background,
    /// Fractional safe-area margin around the foreground (0.0-0.5).
    /// At render time the foreground is rasterized at
    /// `size * (1 - 2*padding)` and centered, giving the system
    /// mask room to crop without clipping the glyph. Default
    /// applied in `Source::from_block` rather than at parse time
    /// so override-vs-base merge can leave it unset.
    padding: f32,
}

/// Apple's HIG-ish margin: glyph occupies central ~80% of the
/// canvas. Same value works for Android adaptive icons, where the
/// safe zone is 72/108 dp ≈ 67% — our 80% glyph fits inside the
/// 67% zone after the system mask, with a little visible
/// background ring around it.
const DEFAULT_FOREGROUND_PADDING: f32 = 0.10;

impl Source {
    /// Build a renderable source from the resolved block. Picks
    /// the standalone path when `source` is set, the composite path
    /// when both `foreground` and `background` are set, and errors
    /// otherwise (the caller's TOML doesn't carry enough to draw
    /// anything).
    pub fn from_block(icon: &IconBlock) -> Result<Self> {
        if let Some(source_path) = icon.source.as_deref() {
            return Ok(Source::Standalone(load_standalone(source_path)?));
        }
        match (icon.foreground.as_deref(), icon.background.as_ref()) {
            (Some(fg_path), Some(bg)) => Ok(Source::Composite(CompositeSource {
                foreground: load_standalone(fg_path)?,
                background: bg.clone(),
                padding: icon
                    .foreground_padding
                    .unwrap_or(DEFAULT_FOREGROUND_PADDING),
            })),
            (Some(_), None) => Err(anyhow!(
                "icon block has `foreground` but no `background` — either \
                 add a background (e.g. `background = \"#ffffff\"`) or use \
                 `source` for a standalone icon",
            )),
            (None, Some(_)) => Err(anyhow!(
                "icon block has `background` but no `foreground` — add \
                 `foreground = \"path/to/glyph.svg\"` to draw something \
                 over the backdrop",
            )),
            (None, None) => Err(anyhow!(
                "icon block has no `source`, `foreground`, or `background` \
                 to render — declare at least `source = \"…\"`",
            )),
        }
    }

    pub fn render_png(&self, size: u32) -> Result<Vec<u8>> {
        match self {
            Source::Standalone(s) => render_standalone_png(s, size),
            Source::Composite(c) => render_composite_png(c, size),
        }
    }
}

fn load_standalone(path: &Path) -> Result<StandaloneSource> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("read icon source {}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("svg") => {
            let mut opts = resvg::usvg::Options::default();
            if let Some(parent) = path.parent() {
                opts.resources_dir = Some(parent.to_path_buf());
            }
            let tree = resvg::usvg::Tree::from_data(&bytes, &opts)
                .with_context(|| format!("parse SVG {}", path.display()))?;
            Ok(StandaloneSource::Svg(tree))
        }
        Some("png") | Some("jpg") | Some("jpeg") => {
            let img = image::load_from_memory(&bytes)
                .with_context(|| format!("decode raster {}", path.display()))?;
            Ok(StandaloneSource::Raster(img))
        }
        other => Err(anyhow!(
            "unsupported icon source extension {:?}; expected .svg, .png, or .jpg/.jpeg ({})",
            other,
            path.display()
        )),
    }
}

fn render_standalone_png(src: &StandaloneSource, size: u32) -> Result<Vec<u8>> {
    let pixmap = rasterize_standalone(src, size)?;
    pixmap.encode_png().context("encode PNG")
}

fn rasterize_standalone(src: &StandaloneSource, size: u32) -> Result<Pixmap> {
    let mut pixmap =
        Pixmap::new(size, size).ok_or_else(|| anyhow!("allocate {size}x{size} pixmap"))?;
    match src {
        StandaloneSource::Svg(tree) => {
            let svg_size = tree.size();
            let sx = size as f32 / svg_size.width();
            let sy = size as f32 / svg_size.height();
            // Uniform scale + center: a non-square SVG renders into
            // a square canvas with transparent padding rather than
            // getting stretched.
            let scale = sx.min(sy);
            let dx = (size as f32 - svg_size.width() * scale) * 0.5;
            let dy = (size as f32 - svg_size.height() * scale) * 0.5;
            let transform = Transform::from_translate(dx, dy).pre_scale(scale, scale);
            resvg::render(tree, transform, &mut pixmap.as_mut());
        }
        StandaloneSource::Raster(img) => {
            let resized = img.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
            // image::DynamicImage is straight-alpha RGBA; tiny-skia
            // pixmaps are premultiplied. We copy through a
            // premultiply step so the composite blends correctly
            // for translucent rasters.
            let rgba = resized.to_rgba8();
            let data = pixmap.data_mut();
            for (i, p) in rgba.pixels().enumerate() {
                let [r, g, b, a] = p.0;
                let a_f = a as f32 / 255.0;
                data[i * 4] = (r as f32 * a_f).round() as u8;
                data[i * 4 + 1] = (g as f32 * a_f).round() as u8;
                data[i * 4 + 2] = (b as f32 * a_f).round() as u8;
                data[i * 4 + 3] = a;
            }
        }
    }
    Ok(pixmap)
}

/// Render JUST the background fill at `size × size`. Used by the
/// Android adaptive-icon pipeline to emit the standalone background
/// layer; the system masks the foreground over it at runtime.
pub(crate) fn render_background_only_png(
    icon: &IconBlock,
    size: u32,
) -> Result<Vec<u8>> {
    let bg = icon.background.as_ref().ok_or_else(|| {
        anyhow!(
            "adaptive Android icon needs a `background` (color or gradient); \
             none is set in the resolved block",
        )
    })?;
    let mut canvas =
        Pixmap::new(size, size).ok_or_else(|| anyhow!("allocate {size}x{size} pixmap"))?;
    paint_background(&mut canvas, bg, size)?;
    canvas
        .encode_png()
        .context("encode adaptive background PNG")
}

/// Render JUST the foreground glyph at `size × size`, centered with
/// `padding` (fractional) of transparent margin on each side. Used
/// by the Android adaptive-icon pipeline so the system can mask the
/// foreground over the background separately. The padding here is
/// distinct from `IconBlock::foreground_padding` — adaptive icons
/// have a fixed safe-zone the launcher honors (66/108 dp ≈ 19.4%),
/// and the caller passes that value explicitly.
pub(crate) fn render_foreground_only_png(
    icon: &IconBlock,
    size: u32,
    padding: f32,
) -> Result<Vec<u8>> {
    let fg_path = icon.foreground.as_deref().ok_or_else(|| {
        anyhow!(
            "adaptive Android icon needs a `foreground` SVG/PNG; \
             none is set in the resolved block",
        )
    })?;
    let fg_source = load_standalone(fg_path)?;
    let mut canvas =
        Pixmap::new(size, size).ok_or_else(|| anyhow!("allocate {size}x{size} pixmap"))?;
    let padding_px = (size as f32 * padding).round() as i32;
    let inner = ((size as i32) - 2 * padding_px).max(1) as u32;
    let glyph = rasterize_standalone(&fg_source, inner)?;
    canvas.draw_pixmap(
        padding_px,
        padding_px,
        glyph.as_ref(),
        &resvg::tiny_skia::PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    canvas.encode_png().context("encode adaptive foreground PNG")
}

fn render_composite_png(c: &CompositeSource, size: u32) -> Result<Vec<u8>> {
    let mut canvas =
        Pixmap::new(size, size).ok_or_else(|| anyhow!("allocate {size}x{size} pixmap"))?;
    paint_background(&mut canvas, &c.background, size)?;

    // Rasterize the foreground at its reduced target size directly
    // — the SVG rasterizer renders crisp at that scale, which is
    // strictly higher quality than rasterizing at full canvas size
    // and then downscaling. The result is offset by `padding * size`
    // so the foreground lands centered.
    let padding_px = (size as f32 * c.padding).round() as i32;
    let fg_size = ((size as i32) - 2 * padding_px).max(1) as u32;
    let foreground = rasterize_standalone(&c.foreground, fg_size)?;
    canvas.draw_pixmap(
        padding_px,
        padding_px,
        foreground.as_ref(),
        &resvg::tiny_skia::PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    canvas.encode_png().context("encode composite PNG")
}

fn paint_background(canvas: &mut Pixmap, bg: &Background, size: u32) -> Result<()> {
    let rect = Rect::from_xywh(0.0, 0.0, size as f32, size as f32)
        .ok_or_else(|| anyhow!("invalid {size}x{size} rect"))?;
    let mut paint = Paint::default();
    paint.anti_alias = true;
    match bg {
        Background::Color(hex) => {
            paint.set_color(parse_color(hex)?);
        }
        Background::Gradient(g) => match g {
            Gradient::Linear { angle_deg, stops } => {
                let (start, end) = linear_endpoints(size, *angle_deg);
                let shader_stops: Vec<GradientStop> = stops
                    .iter()
                    .map(|s| {
                        let c = parse_color(&s.color)?;
                        Ok(GradientStop::new(s.offset, c))
                    })
                    .collect::<Result<Vec<_>>>()
                    .context("linear gradient stop colors")?;
                let shader = LinearGradient::new(
                    start,
                    end,
                    shader_stops,
                    SpreadMode::Pad,
                    Transform::identity(),
                )
                .ok_or_else(|| anyhow!("linear gradient construction failed"))?;
                paint.shader = shader;
            }
        },
    }
    canvas.fill_rect(rect, &paint, Transform::identity(), None);
    Ok(())
}

/// Map a CSS-style angle (0 = up, increasing clockwise) to the
/// gradient line's start and end points. The line passes through
/// the canvas center; its half-length is the canvas half-diagonal
/// so any angle covers the full bounding box.
fn linear_endpoints(size: u32, angle_deg: f32) -> (Point, Point) {
    let theta = angle_deg.to_radians();
    let dx = theta.sin();
    let dy = -theta.cos();
    let half = size as f32 * 0.5;
    let r = size as f32 * std::f32::consts::FRAC_1_SQRT_2;
    (
        Point::from_xy(half - dx * r, half - dy * r),
        Point::from_xy(half + dx * r, half + dy * r),
    )
}

/// Parse `#RGB`, `#RGBA`, `#RRGGBB`, or `#RRGGBBAA`. Tiny-skia uses
/// straight-alpha [`Color`]; the surrounding paint pipeline handles
/// premultiplication when drawn.
pub(crate) fn parse_color(s: &str) -> Result<Color> {
    let hex = s.strip_prefix('#').ok_or_else(|| {
        anyhow!("color {s:?} must start with `#` (e.g. \"#EFDD74\")")
    })?;
    let (r, g, b, a) = match hex.len() {
        3 => (
            dup_nibble(hex_byte(hex, 0)?)?,
            dup_nibble(hex_byte(hex, 1)?)?,
            dup_nibble(hex_byte(hex, 2)?)?,
            255,
        ),
        4 => (
            dup_nibble(hex_byte(hex, 0)?)?,
            dup_nibble(hex_byte(hex, 1)?)?,
            dup_nibble(hex_byte(hex, 2)?)?,
            dup_nibble(hex_byte(hex, 3)?)?,
        ),
        6 => (
            byte_pair(hex, 0)?,
            byte_pair(hex, 2)?,
            byte_pair(hex, 4)?,
            255,
        ),
        8 => (
            byte_pair(hex, 0)?,
            byte_pair(hex, 2)?,
            byte_pair(hex, 4)?,
            byte_pair(hex, 6)?,
        ),
        _ => {
            return Err(anyhow!(
                "color {s:?} must be #RGB, #RGBA, #RRGGBB, or #RRGGBBAA"
            ))
        }
    };
    Ok(Color::from_rgba8(r, g, b, a))
}

fn hex_byte(s: &str, idx: usize) -> Result<u8> {
    let b = s.as_bytes()[idx];
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        other => Err(anyhow!(
            "invalid hex digit {:?} in color",
            other as char
        )),
    }
}

fn dup_nibble(n: u8) -> Result<u8> {
    Ok((n << 4) | n)
}

fn byte_pair(s: &str, idx: usize) -> Result<u8> {
    let hi = hex_byte(s, idx)?;
    let lo = hex_byte(s, idx + 1)?;
    Ok((hi << 4) | lo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_color_round_trips_six_digit() {
        let c = parse_color("#EFDD74").unwrap();
        assert_eq!((c.red() * 255.0).round() as u8, 0xEF);
        assert_eq!((c.green() * 255.0).round() as u8, 0xDD);
        assert_eq!((c.blue() * 255.0).round() as u8, 0x74);
        assert_eq!((c.alpha() * 255.0).round() as u8, 255);
    }

    #[test]
    fn parse_color_handles_short_form() {
        let c = parse_color("#fa0").unwrap();
        assert_eq!((c.red() * 255.0).round() as u8, 0xFF);
        assert_eq!((c.green() * 255.0).round() as u8, 0xAA);
        assert_eq!((c.blue() * 255.0).round() as u8, 0x00);
    }

    #[test]
    fn parse_color_rejects_missing_hash() {
        assert!(parse_color("EFDD74").is_err());
    }

    #[test]
    fn linear_endpoints_180_top_to_bottom() {
        // CSS-convention 180° = "to bottom": start at top, end at
        // bottom. The Y axis grows downward in pixmap space, so the
        // start point's y < end point's y.
        let (start, end) = linear_endpoints(100, 180.0);
        assert!(start.y < end.y, "180° start={start:?} end={end:?}");
        assert!((start.x - 50.0).abs() < 0.5);
        assert!((end.x - 50.0).abs() < 0.5);
    }

    #[test]
    fn linear_endpoints_90_left_to_right() {
        let (start, end) = linear_endpoints(100, 90.0);
        assert!(start.x < end.x);
        assert!((start.y - 50.0).abs() < 0.5);
        assert!((end.y - 50.0).abs() < 0.5);
    }
}
