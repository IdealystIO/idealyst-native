//! Shared CPU-backend support for [`DrawOp::Glyphs`] runs.
//!
//! The GPU renderer (`canvas-vello`) drives vello's glyph pipeline directly. The
//! CPU backends (CoreGraphics, Canvas2D, `android.graphics`) have no glyph
//! engine, so they **outline** each glyph from the run's font bytes and fill it —
//! producing the *same* geometry as the GPU path, because both outline at
//! `upem = 1000` with hinting off (CLAUDE.md §7). This module is that shared
//! expansion; each backend calls [`expand_run`] and replays the returned
//! `Fill` ops through its existing fill path.

use canvas_core::{DrawOp, FillRule, FontResource, Paint, Path, PathSeg, PositionedGlyph};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// The em a [`DrawOp::Glyphs`] run is normalized to — must match the GPU path's
/// `GLYPH_UPEM` and `canvas_core::PositionedGlyph`'s contract (each glyph affine
/// places a 1000-upem outline).
const GLYPH_UPEM: f32 = 1000.0;

/// Expand a glyph run into a flat list of `Save · Transform · Fill · Restore`
/// ops — one quartet per glyph that has an outline (whitespace glyphs and
/// unresolvable ids are skipped). Returns empty if the font bytes don't parse.
///
/// Each glyph's outline is taken at `upem = 1000` (so it lands in the exact space
/// the run's per-glyph affine expects) and filled non-zero with the run's paint,
/// wrapped in the glyph's transform — structurally identical to the
/// `SceneDevice`'s own outline fallback and to the vello glyph render.
pub(crate) fn expand_run(
    font: &FontResource,
    glyphs: &[PositionedGlyph],
    paint: &Paint,
) -> Vec<DrawOp> {
    let Ok(font_ref) = FontRef::from_index(font.data.as_slice(), font.index) else {
        return Vec::new();
    };
    let outlines = font_ref.outline_glyphs();

    let mut ops = Vec::with_capacity(glyphs.len() * 4);
    for pg in glyphs {
        let Some(glyph) = outlines.get(GlyphId::new(pg.id)) else { continue };
        let mut pen = PathPen::default();
        let settings = DrawSettings::unhinted(Size::new(GLYPH_UPEM), LocationRef::default());
        if glyph.draw(settings, &mut pen).is_err() {
            continue;
        }
        if pen.path.segs.is_empty() {
            continue; // whitespace / empty glyph
        }
        ops.push(DrawOp::Save);
        ops.push(DrawOp::Transform(pg.transform));
        ops.push(DrawOp::Fill { path: pen.path, paint: paint.clone(), fill_rule: FillRule::NonZero });
        ops.push(DrawOp::Restore);
    }
    ops
}

/// A skrifa [`OutlinePen`] that records into a canvas [`Path`]. Coordinates are
/// font-design units (y-up) at the requested em; the glyph's affine handles the
/// flip to logical (y-down) space, exactly as `hayro`'s `outline()` is consumed.
#[derive(Default)]
struct PathPen {
    path: Path,
}

impl OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.segs.push(PathSeg::MoveTo { x, y });
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.segs.push(PathSeg::LineTo { x, y });
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.path.segs.push(PathSeg::QuadTo { cx, cy, x, y });
    }
    fn curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        self.path.segs.push(PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y });
    }
    fn close(&mut self) {
        self.path.segs.push(PathSeg::Close);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::{Color, Transform};

    #[test]
    fn unparseable_font_yields_no_ops() {
        // A run whose font bytes aren't a valid sfnt expands to nothing rather
        // than panicking — the producer's GPU path would also draw nothing.
        let font = FontResource::new(1, 0, vec![0u8; 16]);
        let glyphs = vec![PositionedGlyph::new(3, Transform::IDENTITY)];
        let ops = expand_run(&font, &glyphs, &Paint::solid(Color::new(0, 0, 0, 255)));
        assert!(ops.is_empty());
    }

    #[test]
    fn empty_run_yields_no_ops() {
        let font = FontResource::new(1, 0, vec![0u8; 16]);
        assert!(expand_run(&font, &[], &Paint::solid(Color::new(0, 0, 0, 255))).is_empty());
    }
}
