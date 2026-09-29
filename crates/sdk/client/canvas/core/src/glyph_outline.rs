//! Outlining [`DrawOp::Glyphs`] runs for renderers with no glyph engine.
//!
//! The GPU renderer (`canvas-vello`) draws a glyph run through vello's glyph
//! pipeline. The CPU renderers (`canvas-native`: CoreGraphics, Canvas2D,
//! `android.graphics`, Cairo) have none, so they **outline** each glyph from the
//! run's font bytes and fill it — the *same* geometry as the GPU path, because
//! both outline at `upem = 1000` with hinting off (CLAUDE.md §7). A renderer
//! calls [`expand_glyph_run`] and replays the returned `Fill` ops.
//!
//! # Why the outliner is installed, not called
//!
//! Outlining needs a font parser (skrifa + read-fonts, ~400 KB of wasm). If the
//! renderer called it directly, every app that draws ANY canvas — a chart —
//! would ship it, because a renderer's op interpreter handles every op kind in
//! one function. Only glyph runs need it, and a glyph run cannot exist without
//! a [`FontResource`]. So the parser is linked through the two ways a
//! `FontResource` comes into being — [`FontResource::new`] and deserializing
//! one (a scene painted in another process) — which install it here; the
//! renderer reaches it only through this hook. An app that never makes a font
//! (charts only) never links the parser; an app whose glyph producer is a lazy
//! component (a PDF viewer) links it into that component's module.
//!
//! The one way around the install is building a `FontResource` with a struct
//! literal (its fields are public). A renderer that meets such a run warns once
//! and draws no text rather than failing silently.

use std::sync::OnceLock;

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

use crate::{DrawOp, FillRule, FontResource, Paint, Path, PathSeg, PositionedGlyph};

/// The em a glyph run is normalized to — must match the GPU path's
/// `GLYPH_UPEM` and [`PositionedGlyph`]'s contract (each glyph affine places a
/// 1000-upem outline).
pub const GLYPH_UPEM: f32 = 1000.0;

type Outliner = fn(&FontResource, &[PositionedGlyph], &Paint) -> Vec<DrawOp>;

static OUTLINER: OnceLock<Outliner> = OnceLock::new();

/// Link and install the outliner. Called by every path that creates a
/// [`FontResource`]; idempotent and cheap after the first call.
pub(crate) fn install() {
    OUTLINER.get_or_init(|| outline_run);
}

/// Expand a glyph run into `Save · Transform · Fill · Restore` quartets — one
/// per glyph with an outline (whitespace and unresolvable ids are skipped).
/// Empty if the font bytes don't parse, or if no outliner was installed (see
/// the module docs).
pub fn expand_glyph_run(font: &FontResource, glyphs: &[PositionedGlyph], paint: &Paint) -> Vec<DrawOp> {
    match OUTLINER.get() {
        Some(outline) => outline(font, glyphs, paint),
        None => {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                eprintln!(
                    "[canvas] a glyph run's FontResource was built with a struct literal, so no \
                     glyph outliner is installed and its text will not draw; build fonts with \
                     `FontResource::new`"
                );
            });
            Vec::new()
        }
    }
}

/// Each glyph's outline is taken at `upem = 1000` (so it lands in the exact space
/// the run's per-glyph affine expects) and filled non-zero with the run's paint,
/// wrapped in the glyph's transform — structurally identical to the vello glyph
/// render.
fn outline_run(font: &FontResource, glyphs: &[PositionedGlyph], paint: &Paint) -> Vec<DrawOp> {
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
    use crate::{Color, Transform};

    pub(crate) const INTER: &[u8] =
        include_bytes!("../../../../../gpu-backend/engine/assets/fonts/Inter-Regular.ttf");

    fn black() -> Paint {
        Paint::solid(Color::new(0, 0, 0, 255))
    }

    #[test]
    fn a_glyph_outlines_into_a_filled_path_at_its_transform() {
        let font = FontResource::new(1, 0, INTER.to_vec());
        let id = FontRef::new(INTER).unwrap().charmap().map('H').unwrap().to_u32();
        let at = Transform::translate(10.0, 20.0);
        let ops = expand_glyph_run(&font, &[PositionedGlyph::new(id, at)], &black());
        assert_eq!(ops.len(), 4, "Save · Transform · Fill · Restore");
        assert!(matches!(ops[1], DrawOp::Transform(t) if t == at));
        let DrawOp::Fill { path, fill_rule, .. } = &ops[2] else { panic!("{:?}", ops[2]) };
        assert_eq!(*fill_rule, FillRule::NonZero);
        assert!(path.segs.len() > 4, "an H has a real outline");
    }

    #[test]
    fn whitespace_draws_nothing() {
        let font = FontResource::new(1, 0, INTER.to_vec());
        let space = FontRef::new(INTER).unwrap().charmap().map(' ').unwrap().to_u32();
        assert!(expand_glyph_run(&font, &[PositionedGlyph::new(space, Transform::IDENTITY)], &black()).is_empty());
    }

    #[test]
    fn unparseable_font_yields_no_ops() {
        // Font bytes that aren't a valid sfnt expand to nothing rather than
        // panicking — the GPU path would also draw nothing.
        let font = FontResource::new(1, 0, vec![0u8; 16]);
        let glyphs = vec![PositionedGlyph::new(3, Transform::IDENTITY)];
        assert!(expand_glyph_run(&font, &glyphs, &black()).is_empty());
    }

    #[test]
    fn empty_run_yields_no_ops() {
        let font = FontResource::new(1, 0, vec![0u8; 16]);
        assert!(expand_glyph_run(&font, &[], &black()).is_empty());
    }
}
