//! [`SceneDevice`] — a [`hayro_interpret::Device`] that records every PDF
//! drawing instruction into a renderer-agnostic [`canvas_core::Scene`].
//!
//! The scene it produces is in **page-point logical coordinates** (origin
//! top-left, y-down — the page's y-up space is flipped by the page's
//! `initial_transform`, applied by the caller in [`crate::render_page`]). The
//! PDF SDK then scales the whole scene to fit the canvas box; the canvas
//! renderer scales again for device-pixel ratio. So one interpretation works at
//! every zoom and on every backend.
//!
//! # How PDF ops map onto the scene
//!
//! | hayro `Device` call        | scene op(s)                                  |
//! |----------------------------|----------------------------------------------|
//! | `draw_path` (fill)         | `Save` · `Transform` · `Fill` · `Restore`    |
//! | `draw_path` (stroke)       | `Save` · `Transform` · `Stroke` · `Restore`  |
//! | `draw_glyph` (outline+sfnt)| buffered into a [`DrawOp::Glyphs`] run       |
//! | `draw_glyph` (Type1/no sfnt)| outline → `Fill`/`Stroke`                    |
//! | `draw_glyph` (Type3)       | `Type3Glyph::interpret` → nested path ops    |
//! | `draw_image`               | `Save` · `Transform` · `Image` · `Restore`   |
//! | `push/pop_clip_path`       | `Save` + `Clip` … `Restore`                  |
//! | `push/pop_transparency_group`| nested op-sink → `Layer` (or flattened)    |
//!
//! # Why a wrapping `Transform` per draw, but a baked affine per glyph
//!
//! Every `draw_path`/`draw_image` carries its own full affine. Emitting
//! `Save·Transform·op·Restore` mirrors the reference renderer's
//! `set_transform(t); fill_path(p)` exactly — including stroke widths scaling
//! with the CTM — and keeps the active clip (an outer `Save`) untouched. Glyphs
//! are different: a [`DrawOp::Glyphs`] run carries each glyph's *baked*
//! `transform · glyph_transform` affine (see [`canvas_core::PositionedGlyph`]),
//! so the renderer drives its glyph pipeline once per run with one font upload
//! rather than wrapping each glyph in its own transform.

use std::collections::HashMap;

use canvas_core::{
    BlendMode, Color, FillRule, FontResource, LineCap, LineJoin, Paint, Path, PathSeg,
    PositionedGlyph, Scene, Stroke, Transform,
};
use hayro_interpret::font::Glyph;
use hayro_interpret::hayro_syntax::object::ObjectIdentifier;
use hayro_interpret::pattern::{Pattern, ShadingPattern};
use hayro_interpret::MaskType;
use hayro_interpret::{
    BlendMode as PdfBlend, ClipPath, Device, FillRule as PdfFill, GlyphDrawMode, Image,
    Paint as PdfPaint, PathDrawMode, SoftMask, StrokeProps,
};
use kurbo::{Affine, BezPath, PathEl, Point};

/// Glyph-space is normalized to 1000 units-per-em by `hayro`'s `outline()`, and
/// a [`DrawOp::Glyphs`] run is rendered at this nominal em (the renderer's
/// `transform` carries the real on-page scale). Kept here as the single source
/// of the invariant the canvas renderers also assume.
pub const GLYPH_UPEM: f32 = 1000.0;

/// Drawing instructions the bridge cannot represent exactly yet — surfaced so a
/// caller (and the docs) can see what a rendered page approximated, instead of
/// silently dropping it (CLAUDE.md §7 / "no silent caps").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Warnings {
    /// **Tiling** pattern fills drawn as nothing — shading patterns (gradients)
    /// ARE rendered; only tiling patterns aren't modeled yet.
    pub pattern_paints: u32,
    /// **Alpha**-type soft masks (`/Alpha`) — rendered, but via the luminance
    /// path (vello has no alpha-mask primitive), so they're approximate.
    /// Luminosity masks (the common case) render exactly and aren't counted.
    pub soft_masks: u32,
    /// Blend modes the canvas can't represent — always `0` now (all 16 PDF
    /// separable + non-separable blends map 1:1 onto the canvas set). Kept for
    /// API stability + future modes.
    pub unsupported_blends: u32,
}

/// A [`hayro_interpret::Device`] that records into a [`Scene`].
pub struct SceneDevice {
    /// Op-sink stack. The top sink receives ops; a transparency group pushes a
    /// fresh sink and pops it into a [`DrawOp::Layer`] on the parent. Starts with
    /// the root sink (index 0), which becomes the finished scene's ops.
    sinks: Vec<Vec<canvas_core::DrawOp>>,
    /// The in-progress glyph run, flushed on any op boundary or key change.
    pending: Option<PendingRun>,
    /// Current blend mode (from `set_blend_mode`), applied to emitted paints.
    blend: BlendMode,
    /// The active graphics-state soft mask (from `set_soft_mask`), if any. Every
    /// draw made while this is set is wrapped in a [`DrawOp::MaskGroup`]. The
    /// mask's ops are rendered once and cached here, keyed by its object id, so a
    /// mask shared across many draws (masked text) isn't re-interpreted per draw.
    active_mask: Option<ActiveMask>,
    /// Monotonic id for transient transparency-group layers.
    next_layer_id: u32,
    /// Per-font raw bytes + id, keyed by `font_cache_key`, so a run reuses one
    /// `FontResource` (an `Arc` clone) instead of re-extracting bytes per glyph.
    fonts: HashMap<u128, FontResource>,
    /// Compositing params for each open transparency group, parallel to the part
    /// of `sinks` above the root.
    group_meta: Vec<GroupMeta>,
    /// Approximation counters (see [`Warnings`]).
    warnings: Warnings,
}

/// A glyph run being accumulated before it becomes a [`DrawOp::Glyphs`].
struct PendingRun {
    font_key: u128,
    paint: Paint,
    glyphs: Vec<PositionedGlyph>,
}

/// The active soft mask: its rendered ops (cached) + how it's keyed.
struct ActiveMask {
    /// Object id of the mask, to detect when `set_soft_mask` swaps it.
    id: ObjectIdentifier,
    /// Mask on rendered luminance (`/Luminosity`) vs alpha (`/Alpha`).
    luminance: bool,
    /// The mask's drawing ops (rendered once via `SoftMask::interpret`).
    ops: Vec<canvas_core::DrawOp>,
}

impl Default for SceneDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneDevice {
    /// A fresh device with a single root op-sink.
    pub fn new() -> Self {
        Self {
            sinks: vec![Vec::new()],
            pending: None,
            blend: BlendMode::Normal,
            active_mask: None,
            next_layer_id: 0,
            fonts: HashMap::new(),
            group_meta: Vec::new(),
            warnings: Warnings::default(),
        }
    }

    /// Consume the device, returning the recorded [`Scene`] and any
    /// approximation [`Warnings`]. The root sink is the scene; nested
    /// group-sinks must already be popped (balanced push/pop).
    pub fn finish(mut self) -> (Scene, Warnings) {
        self.flush_run();
        debug_assert_eq!(self.sinks.len(), 1, "unbalanced transparency-group stack");
        let ops = self.sinks.pop().unwrap_or_default();
        (Scene::from_ops(ops), self.warnings)
    }

    /// The sink currently receiving ops.
    fn sink(&mut self) -> &mut Vec<canvas_core::DrawOp> {
        self.sinks.last_mut().expect("at least the root sink")
    }

    /// Push a non-glyph op, flushing any pending glyph run first so draw order is
    /// preserved (a glyph run must not float past a later path/image/clip).
    fn push_op(&mut self, op: canvas_core::DrawOp) {
        self.flush_run();
        self.sink().push(op);
    }

    /// Emit the accumulated glyph run, if any, as one [`DrawOp::Glyphs`] (mask-
    /// wrapped if a soft mask is active).
    fn flush_run(&mut self) {
        if let Some(run) = self.pending.take() {
            if !run.glyphs.is_empty() {
                if let Some(font) = self.fonts.get(&run.font_key).cloned() {
                    let op = canvas_core::DrawOp::Glyphs { font, glyphs: run.glyphs, paint: run.paint };
                    self.push_masked(vec![op]);
                }
            }
        }
    }

    /// Push a self-contained draw (its full op sequence). Under an active soft
    /// mask, wrap it in a [`DrawOp::MaskGroup`] so the mask modulates it; else
    /// append the ops directly. Does NOT flush the pending glyph run (callers
    /// that interleave with other ops use [`emit_draw`](Self::emit_draw)).
    fn push_masked(&mut self, content: Vec<canvas_core::DrawOp>) {
        // Clone the mask ops into a local first to release the `self` borrow.
        let masked = self.active_mask.as_ref().map(|m| (m.ops.clone(), m.luminance));
        match masked {
            Some((mask, luminance)) => self.sink().push(canvas_core::DrawOp::MaskGroup {
                content,
                mask,
                luminance,
                alpha: 1.0,
                blend: BlendMode::Normal,
            }),
            None => self.sink().extend(content),
        }
    }

    /// Flush any pending glyph run, then [`push_masked`](Self::push_masked) a
    /// draw — the entry point every path/image/shading draw uses, so order and
    /// masking are handled uniformly.
    fn emit_draw(&mut self, content: Vec<canvas_core::DrawOp>) {
        self.flush_run();
        self.push_masked(content);
    }

    /// Resolve a hayro paint to a canvas [`Paint`] at the current blend mode.
    /// Shading patterns are handled earlier (in `draw_path`); a `Pattern` reaching
    /// here is a tiling pattern (not modeled) → transparent + counted.
    fn resolve_paint(&mut self, paint: &PdfPaint) -> Paint {
        match paint {
            PdfPaint::Color(c) => {
                let [r, g, b, a] = c.to_rgba().to_rgba8();
                Paint::solid(Color::new(r, g, b, a)).blend(self.blend)
            }
            PdfPaint::Pattern(_) => {
                self.warnings.pattern_paints += 1;
                Paint::solid(Color::new(0, 0, 0, 0)).blend(self.blend)
            }
        }
    }

    /// Fill `path` with a shading pattern: render the shading to a texture over
    /// the path's device-space box and blit it clipped to the path. Returns
    /// `false` (caller falls back) only for a degenerate box. The path is first
    /// mapped to device space (CTM baked in) so the emitted clip + image live in
    /// logical space directly — no per-op `Transform` wrap needed.
    fn fill_shading(
        &mut self,
        path: &BezPath,
        transform: Affine,
        shading: &ShadingPattern,
        fill_rule: PdfFill,
    ) -> bool {
        let mut device_path = path.clone();
        device_path.apply_affine(transform);
        let Some((src, dst)) = crate::shading::render(shading, &device_path) else {
            return false;
        };
        let clip = bez_to_path(&device_path);
        let blend = self.blend;
        self.emit_draw(vec![
            canvas_core::DrawOp::Save,
            canvas_core::DrawOp::Clip { path: clip, fill_rule: map_fill(fill_rule) },
            canvas_core::DrawOp::Image { image: std::sync::Arc::new(src), dst, alpha: 1.0, blend },
            canvas_core::DrawOp::Restore,
        ]);
        true
    }
}

impl<'a> Device<'a> for SceneDevice {
    fn set_soft_mask(&mut self, mask: Option<SoftMask<'a>>) {
        self.flush_run();
        match mask {
            None => self.active_mask = None,
            Some(m) => {
                let id = m.id();
                // hayro re-sets the same mask before every draw; only (re)render
                // when the mask actually changes.
                if self.active_mask.as_ref().map(|a| a.id) == Some(id) {
                    return;
                }
                // Alpha masks key on the rendered alpha; we render via vello's
                // luminance mask, so flag alpha as an approximation (counted).
                let luminance = matches!(m.mask_type(), MaskType::Luminosity);
                if !luminance {
                    self.warnings.soft_masks += 1;
                }
                // Render the mask's content into a sub-device → its ops.
                let mut sub = SceneDevice::new();
                m.interpret(&mut sub);
                let (mask_scene, _) = sub.finish();
                self.active_mask = Some(ActiveMask { id, luminance, ops: mask_scene.ops().to_vec() });
            }
        }
    }

    fn set_blend_mode(&mut self, blend_mode: PdfBlend) {
        self.flush_run();
        self.blend = map_blend(blend_mode);
    }

    fn draw_path(
        &mut self,
        path: &BezPath,
        transform: Affine,
        paint: &PdfPaint<'a>,
        draw_mode: &PathDrawMode,
    ) {
        // A shading-pattern FILL is rendered by sampling the shading to a texture
        // clipped to the path (gradients/shadings — see `shading.rs`), not as a
        // flat paint.
        if let (PathDrawMode::Fill(fr), PdfPaint::Pattern(p)) = (draw_mode, paint) {
            if let Pattern::Shading(sp) = &**p {
                if self.fill_shading(path, transform, sp, *fr) {
                    return;
                }
            }
        }

        let canvas_path = bez_to_path(path);
        let paint = self.resolve_paint(paint);
        let t = affine_to_transform(transform);
        let draw = match draw_mode {
            PathDrawMode::Fill(fill_rule) => {
                canvas_core::DrawOp::Fill { path: canvas_path, paint, fill_rule: map_fill(*fill_rule) }
            }
            PathDrawMode::Stroke(props) => {
                canvas_core::DrawOp::Stroke { path: canvas_path, paint, stroke: conv_stroke(props) }
            }
        };
        self.emit_draw(vec![
            canvas_core::DrawOp::Save,
            canvas_core::DrawOp::Transform(t),
            draw,
            canvas_core::DrawOp::Restore,
        ]);
    }

    fn draw_glyph(
        &mut self,
        glyph: &Glyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        paint: &PdfPaint<'a>,
        draw_mode: &GlyphDrawMode,
    ) {
        // Invisible text (render mode 3 — OCR underlay) is for extraction only.
        if matches!(draw_mode, GlyphDrawMode::Invisible) {
            return;
        }
        // The full upem-1000 outline → device affine, per the reference's
        // `fill_path(outline, transform * glyph_transform, …)`.
        let device = transform * glyph_transform;

        match glyph {
            Glyph::Outline(o) => {
                // GPU glyph-run fast path: an sfnt/CFF program skrifa can load
                // (Type1 returns `None`) + a plain fill. Buffer into the current
                // run keyed by font + paint.
                if matches!(draw_mode, GlyphDrawMode::Fill) {
                    if let Some(fd) = o.font_data() {
                        let key = fd.cache_key;
                        let resolved = self.resolve_paint(paint);
                        self.buffer_glyph(key, &fd, o.glyph_id().to_u32(), device, resolved);
                        return;
                    }
                }
                // Fallback: outline → path op (Type1 fills, every stroke). The
                // outline is in upem-1000 space; place it with `device`.
                let path = bez_to_path(&o.outline());
                let paint = self.resolve_paint(paint);
                let t = affine_to_transform(device);
                let draw = match draw_mode {
                    GlyphDrawMode::Stroke(props) => {
                        canvas_core::DrawOp::Stroke { path, paint, stroke: conv_stroke(props) }
                    }
                    _ => canvas_core::DrawOp::Fill { path, paint, fill_rule: FillRule::NonZero },
                };
                self.emit_draw(vec![
                    canvas_core::DrawOp::Save,
                    canvas_core::DrawOp::Transform(t),
                    draw,
                    canvas_core::DrawOp::Restore,
                ]);
            }
            Glyph::Type3(t3) => {
                // Type3 glyphs are PDF content streams: re-drive ourselves so
                // their drawing instructions become ordinary path/image ops.
                self.flush_run();
                t3.interpret(self, transform, glyph_transform, paint);
            }
        }
    }

    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        if let Some((src, dst, eff)) = crate::image::convert(image, transform) {
            let t = affine_to_transform(eff);
            let blend = self.blend;
            self.emit_draw(vec![
                canvas_core::DrawOp::Save,
                canvas_core::DrawOp::Transform(t),
                canvas_core::DrawOp::Image { image: std::sync::Arc::new(src), dst, alpha: 1.0, blend },
                canvas_core::DrawOp::Restore,
            ]);
        }
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        // Clip paths arrive already in logical/device space (the caller pre-
        // applies the CTM, matching the reference's `push_clip_path_inner`).
        // Scope the clip with Save … Restore so it covers ops until the pop.
        let path = bez_to_path(&clip_path.path);
        self.push_op(canvas_core::DrawOp::Save);
        self.sink().push(canvas_core::DrawOp::Clip { path, fill_rule: map_fill(clip_path.fill) });
    }

    fn pop_clip_path(&mut self) {
        self.push_op(canvas_core::DrawOp::Restore);
    }

    fn push_transparency_group(
        &mut self,
        opacity: f32,
        mask: Option<SoftMask<'a>>,
        blend_mode: PdfBlend,
    ) {
        self.flush_run();
        // Render the group's own soft mask (if any) up front.
        let mask_data = if let Some(m) = mask {
            let luminance = matches!(m.mask_type(), MaskType::Luminosity);
            if !luminance {
                self.warnings.soft_masks += 1;
            }
            let mut sub = SceneDevice::new();
            m.interpret(&mut sub);
            let (s, _) = sub.finish();
            Some((s.ops().to_vec(), luminance))
        } else {
            None
        };
        // Open a fresh op-sink for the group's contents; `pop` composites them.
        self.sinks.push(Vec::new());
        let blend = map_blend(blend_mode);
        self.group_meta.push(GroupMeta { opacity, blend, mask: mask_data });
    }

    fn pop_transparency_group(&mut self) {
        self.flush_run();
        let ops = self.sinks.pop().unwrap_or_default();
        let meta = self.group_meta.pop().unwrap_or(GroupMeta {
            opacity: 1.0,
            blend: BlendMode::Normal,
            mask: None,
        });
        let alpha = meta.opacity.clamp(0.0, 1.0);
        if let Some((mask, luminance)) = meta.mask {
            // A masked group → a MaskGroup carrying its own opacity/blend.
            self.sink().push(canvas_core::DrawOp::MaskGroup {
                content: ops,
                mask,
                luminance,
                alpha,
                blend: meta.blend,
            });
        } else if meta.opacity >= 0.999 && meta.blend == BlendMode::Normal {
            // A trivial group (fully opaque, normal blend) is exactly its
            // contents — splice them in, no layer allocated (the PDF root group
            // is always trivial, so the common case is free).
            self.sink().extend(ops);
        } else {
            let id = self.next_layer_id;
            self.next_layer_id += 1;
            self.sink().push(canvas_core::DrawOp::Layer { id, clear: true, ops, alpha, blend: meta.blend });
        }
    }
}

impl SceneDevice {
    /// Add a glyph to the pending run, opening a new run when the font or paint
    /// changes (which flushes the previous run to preserve color/order).
    fn buffer_glyph(
        &mut self,
        font_key: u128,
        fd: &hayro_interpret::font::OutlineFontData,
        glyph_id: u32,
        device: Affine,
        paint: Paint,
    ) {
        // Register the font's bytes once (id = its cache key, truncated to u64).
        if !self.fonts.contains_key(&font_key) {
            let bytes: Vec<u8> = fd.data.as_ref().as_ref().to_vec();
            self.fonts
                .insert(font_key, FontResource::new(font_key as u64, 0, bytes));
        }

        let same = self
            .pending
            .as_ref()
            .is_some_and(|r| r.font_key == font_key && r.paint == paint);
        if !same {
            self.flush_run();
            self.pending = Some(PendingRun { font_key, paint, glyphs: Vec::new() });
        }
        if let Some(run) = self.pending.as_mut() {
            run.glyphs.push(PositionedGlyph::new(glyph_id, affine_to_transform(device)));
        }
    }
}

/// Per-group compositing parameters, stacked alongside the sink stack.
struct GroupMeta {
    opacity: f32,
    blend: BlendMode,
    /// The group's own soft mask (rendered ops + luminance flag), if any.
    mask: Option<(Vec<canvas_core::DrawOp>, bool)>,
}

// ============================================================================
// Conversions (hayro/kurbo → canvas scene types)
// ============================================================================

/// kurbo [`Affine`] → canvas [`Transform`]. Both are `[a, b, c, d, e, f]` in
/// Canvas2D order (`x' = a·x + c·y + e`), so the coeffs map straight across.
fn affine_to_transform(a: Affine) -> Transform {
    let [a, b, c, d, e, f] = a.as_coeffs();
    Transform { a: a as f32, b: b as f32, c: c as f32, d: d as f32, e: e as f32, f: f as f32 }
}

/// kurbo [`BezPath`] → canvas [`Path`]. Quadratics stay quadratic, cubics cubic.
fn bez_to_path(bez: &BezPath) -> Path {
    let mut segs = Vec::with_capacity(bez.elements().len());
    let p = |pt: Point| (pt.x as f32, pt.y as f32);
    for el in bez.elements() {
        match *el {
            PathEl::MoveTo(a) => {
                let (x, y) = p(a);
                segs.push(PathSeg::MoveTo { x, y });
            }
            PathEl::LineTo(a) => {
                let (x, y) = p(a);
                segs.push(PathSeg::LineTo { x, y });
            }
            PathEl::QuadTo(c, a) => {
                let (cx, cy) = p(c);
                let (x, y) = p(a);
                segs.push(PathSeg::QuadTo { cx, cy, x, y });
            }
            PathEl::CurveTo(c1, c2, a) => {
                let (c1x, c1y) = p(c1);
                let (c2x, c2y) = p(c2);
                let (x, y) = p(a);
                segs.push(PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y });
            }
            PathEl::ClosePath => segs.push(PathSeg::Close),
        }
    }
    Path { segs }
}

/// hayro [`StrokeProps`] → canvas [`Stroke`], including the dash pattern. A dash
/// array of all-zeros (some PDFs use `[0] 0 d` to mean solid) is dropped so it
/// doesn't render an invisible line.
fn conv_stroke(p: &StrokeProps) -> Stroke {
    let dash: Vec<f32> = if p.dash_array.iter().any(|&v| v > 0.0) {
        p.dash_array.to_vec()
    } else {
        Vec::new()
    };
    Stroke {
        width: p.line_width,
        cap: match p.line_cap {
            kurbo::Cap::Butt => LineCap::Butt,
            kurbo::Cap::Round => LineCap::Round,
            kurbo::Cap::Square => LineCap::Square,
        },
        join: match p.line_join {
            kurbo::Join::Miter => LineJoin::Miter,
            kurbo::Join::Round => LineJoin::Round,
            kurbo::Join::Bevel => LineJoin::Bevel,
        },
        miter_limit: p.miter_limit,
        dash,
        dash_offset: p.dash_offset,
    }
}

/// hayro [`PdfFill`] → canvas [`FillRule`].
fn map_fill(f: PdfFill) -> FillRule {
    match f {
        PdfFill::NonZero => FillRule::NonZero,
        PdfFill::EvenOdd => FillRule::EvenOdd,
    }
}

/// hayro [`PdfBlend`] → canvas [`BlendMode`]. PDF's 16 separable + non-separable
/// blend modes map 1:1 onto the canvas blend set (which mirrors the W3C/peniko
/// set), so nothing downgrades. `_` only fires if hayro adds a mode the canvas
/// doesn't have yet.
fn map_blend(b: PdfBlend) -> BlendMode {
    match b {
        PdfBlend::Normal => BlendMode::Normal,
        PdfBlend::Multiply => BlendMode::Multiply,
        PdfBlend::Screen => BlendMode::Screen,
        PdfBlend::Overlay => BlendMode::Overlay,
        PdfBlend::Darken => BlendMode::Darken,
        PdfBlend::Lighten => BlendMode::Lighten,
        PdfBlend::ColorDodge => BlendMode::ColorDodge,
        PdfBlend::ColorBurn => BlendMode::ColorBurn,
        PdfBlend::HardLight => BlendMode::HardLight,
        PdfBlend::SoftLight => BlendMode::SoftLight,
        PdfBlend::Difference => BlendMode::Difference,
        PdfBlend::Exclusion => BlendMode::Exclusion,
        PdfBlend::Hue => BlendMode::Hue,
        PdfBlend::Saturation => BlendMode::Saturation,
        PdfBlend::Color => BlendMode::Color,
        PdfBlend::Luminosity => BlendMode::Luminosity,
    }
}
