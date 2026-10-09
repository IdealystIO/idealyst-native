//! Scene → `vello_cpu` rasterization: the canvas op list drawn on the CPU, for
//! GPUs that can't run vello's compute pipeline (WebGL2, the iOS Simulator,
//! the Android emulator) when canvas content has to land in a GPU texture
//! anyway (the `canvas3d` overlay).
//!
//! It walks the same op list as [`encode_scene`](crate::encode::encode_scene)
//! with the same conversions (paths, brushes, strokes, blend modes are shared
//! — vello 0.9 and vello_cpu 0.3 both speak peniko 0.6 / kurbo 0.13), so the
//! pixels match the GPU path up to rasterizer anti-aliasing (CLAUDE.md §7).
//!
//! Retained layers (`DrawOp::Layer` / `LayerCached`) keep their op logs here
//! across frames — the CPU counterpart of the encoder's retained vello scenes.

use crate::encode::{affine_of, bez_of, brush_of, fill_of, kurbo_stroke, peniko_blend};
use canvas_core::{DrawOp, Scene as CanvasScene};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use vello::kurbo::{Affine, BezPath, Rect, Shape};
use vello::peniko::{BlendMode, Brush, Compose, ImageAlphaType, ImageBrush, ImageSampler, Mix};
use vello_cpu::{Mask, PaintType, Pixmap, RenderContext, Resources};

thread_local! {
    /// `DrawOp::Layer` op logs per id (accumulate across frames, reset on clear).
    static LAYERS: RefCell<HashMap<u32, Vec<DrawOp>>> = RefCell::new(HashMap::new());
    /// `DrawOp::LayerCached` op logs per id (replaced on a dirty bake).
    static CACHED: RefCell<HashMap<u32, Vec<DrawOp>>> = RefCell::new(HashMap::new());
}

/// A clip covering any reachable canvas (isolated-group layers).
fn everything() -> BezPath {
    Rect::new(-1.0e6, -1.0e6, 1.0e6, 1.0e6).to_path(0.1)
}

/// Rasterize `scene` (logical units) at `scale` physical px per unit into a
/// `width`×`height` buffer of **premultiplied** sRGB RGBA8, top-down.
pub fn rasterize_cpu(scene: &CanvasScene, width: u32, height: u32, scale: f64) -> Vec<u8> {
    let (w, h) = (clamp_dim(width), clamp_dim(height));
    let mut ctx = RenderContext::new(w, h);
    encode_ops(scene.ops(), &mut ctx, Affine::scale(scale));
    ctx.flush();
    let mut pixmap = Pixmap::new(w, h);
    ctx.render(&mut pixmap, &mut Resources::new());
    pixmap.data_as_u8_slice().to_vec()
}

fn clamp_dim(d: u32) -> u16 {
    d.clamp(1, u16::MAX as u32) as u16
}

fn paint_of(brush: Brush) -> PaintType {
    match brush {
        Brush::Solid(c) => PaintType::Solid(c),
        Brush::Gradient(g) => PaintType::Gradient(g),
        // `brush_of` never produces image brushes.
        Brush::Image(_) => PaintType::Solid(vello::peniko::Color::TRANSPARENT),
    }
}

fn normal() -> BlendMode {
    BlendMode::new(Mix::Normal, Compose::SrcOver)
}

fn encode_ops(ops: &[DrawOp], ctx: &mut RenderContext, base: Affine) {
    let mut cur = base;
    // (saved transform, clip layers pushed inside this save scope)
    let mut stack: Vec<(Affine, u32)> = Vec::new();
    let mut root_clips: u32 = 0;

    for op in ops {
        match op {
            DrawOp::Save => stack.push((cur, 0)),
            DrawOp::Restore => {
                if let Some((saved, n)) = stack.pop() {
                    for _ in 0..n {
                        ctx.pop_layer();
                    }
                    cur = saved;
                }
            }
            DrawOp::Transform(t) => cur *= affine_of(t),
            DrawOp::Clip { path, fill_rule } => {
                ctx.set_transform(cur);
                ctx.set_fill_rule(fill_of(*fill_rule));
                ctx.push_clip_layer(&bez_of(path));
                match stack.last_mut() {
                    Some(top) => top.1 += 1,
                    None => root_clips += 1,
                }
            }
            DrawOp::Fill { path, paint, fill_rule } => {
                let shape = bez_of(path);
                ctx.set_transform(cur);
                let wrap = peniko_blend(paint.blend);
                if let Some(b) = wrap {
                    // Clip the blend layer to the shape's bounds, like the GPU
                    // encoder: the eraser must only touch what it covers.
                    ctx.push_layer(Some(&shape.bounding_box().to_path(0.1)), Some(b), None, None, None);
                }
                ctx.set_fill_rule(fill_of(*fill_rule));
                ctx.set_paint(paint_of(brush_of(paint)));
                ctx.fill_path(&shape);
                if wrap.is_some() {
                    ctx.pop_layer();
                }
            }
            DrawOp::Stroke { path, paint, stroke } => {
                let shape = bez_of(path);
                ctx.set_transform(cur);
                let wrap = peniko_blend(paint.blend);
                if let Some(b) = wrap {
                    let m = (stroke.width as f64) * 0.5 + 1.0;
                    let bounds = shape.bounding_box().inflate(m, m).to_path(0.1);
                    ctx.push_layer(Some(&bounds), Some(b), None, None, None);
                }
                ctx.set_stroke(kurbo_stroke(stroke));
                ctx.set_paint(paint_of(brush_of(paint)));
                ctx.stroke_path(&shape);
                if wrap.is_some() {
                    ctx.pop_layer();
                }
            }
            DrawOp::Image { image, dst, alpha, blend } => {
                if !image.is_valid() || image.width == 0 || image.height == 0 {
                    continue;
                }
                let (Ok(iw), Ok(ih)) = (u16::try_from(image.width), u16::try_from(image.height)) else {
                    continue;
                };
                let pixmap = Pixmap::from_parts(
                    image.rgba.clone(),
                    iw,
                    ih,
                    vello_cpu::PixelMetadata { alpha_type: ImageAlphaType::Alpha, may_have_transparency: true },
                );
                // Map the image's natural [0,0,w,h] onto `dst` under `cur`.
                let t = cur
                    * Affine::translate((dst.x as f64, dst.y as f64))
                    * Affine::scale_non_uniform(
                        dst.w as f64 / image.width as f64,
                        dst.h as f64 / image.height as f64,
                    );
                let wrap = peniko_blend(*blend);
                if let Some(b) = wrap {
                    ctx.set_transform(cur);
                    let clip = Rect::new(dst.x as f64, dst.y as f64, (dst.x + dst.w) as f64, (dst.y + dst.h) as f64);
                    ctx.push_layer(Some(&clip.to_path(0.1)), Some(b), None, None, None);
                }
                ctx.set_transform(t);
                ctx.set_paint(PaintType::Image(
                    ImageBrush {
                        image: vello_cpu::ImageSource::Pixmap(Arc::new(pixmap)),
                        sampler: ImageSampler::default(),
                    }
                    .with_alpha(*alpha),
                ));
                ctx.fill_rect(&Rect::new(0.0, 0.0, image.width as f64, image.height as f64));
                if wrap.is_some() {
                    ctx.pop_layer();
                }
            }
            DrawOp::Layer { id, clear, ops: nested, alpha, blend } => {
                let log = LAYERS.with(|m| {
                    let mut m = m.borrow_mut();
                    let log = m.entry(*id).or_default();
                    if *clear {
                        log.clear();
                    }
                    log.extend(nested.iter().cloned());
                    log.clone()
                });
                // Isolated group: an eraser inside only cuts the layer's pixels.
                ctx.set_transform(cur);
                ctx.push_layer(
                    Some(&everything()),
                    Some(peniko_blend(*blend).unwrap_or_else(normal)),
                    Some(*alpha),
                    None,
                    None,
                );
                encode_ops(&log, ctx, cur);
                ctx.pop_layer();
            }
            DrawOp::LayerCached { id, dirty, transform, ops: nested, alpha, blend } => {
                let log = CACHED.with(|m| {
                    let mut m = m.borrow_mut();
                    if *dirty {
                        m.insert(*id, nested.clone());
                    }
                    m.get(id).cloned().unwrap_or_default()
                });
                let t = cur * affine_of(transform);
                ctx.set_transform(t);
                ctx.push_layer(
                    Some(&everything()),
                    Some(peniko_blend(*blend).unwrap_or_else(normal)),
                    Some(*alpha),
                    None,
                    None,
                );
                encode_ops(&log, ctx, t);
                ctx.pop_layer();
            }
            DrawOp::Shapes { shapes, blend } => {
                let fills: Vec<DrawOp> = shapes.iter().map(|sh| sh.to_fill_op(*blend)).collect();
                encode_ops(&fills, ctx, cur);
            }
            DrawOp::Glyphs { font, glyphs, paint } => {
                // Outline the run into fills (the same geometry the GPU glyph
                // pipeline draws — see `canvas_core::expand_glyph_run`).
                let fills = canvas_core::expand_glyph_run(font, glyphs, paint);
                encode_ops(&fills, ctx, cur);
            }
            DrawOp::MaskGroup { content, mask, luminance: _, alpha, blend } => {
                // Render the mask ops into their own buffer at the same size and
                // transform, then mask the content group by its luminance (the
                // GPU encoder's luminance-mask layer; alpha masks reuse it there
                // too).
                let (w, h) = (ctx.width(), ctx.height());
                let mut mctx = RenderContext::new(w, h);
                encode_ops(mask, &mut mctx, cur);
                mctx.flush();
                let mut mpix = Pixmap::new(w, h);
                mctx.render(&mut mpix, &mut Resources::new());
                ctx.set_transform(cur);
                ctx.push_layer(
                    None,
                    Some(peniko_blend(*blend).unwrap_or_else(normal)),
                    Some(*alpha),
                    Some(Mask::new_luminance(&mpix)),
                    None,
                );
                encode_ops(content, ctx, cur);
                ctx.pop_layer();
            }
            // Texture layers are composited by the renderers, never encoded.
            DrawOp::Texture { .. } => {}
            _ => {}
        }
    }

    for (_, n) in stack.drain(..) {
        for _ in 0..n {
            ctx.pop_layer();
        }
    }
    for _ in 0..root_clips {
        ctx.pop_layer();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::{Color, Paint, Path};

    fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn fills_land_premultiplied_at_scale() {
        let mut s = CanvasScene::new();
        s.path().add_path(Path::rect(0.0, 0.0, 10.0, 10.0));
        s.fill(Paint::solid(Color::new(255, 0, 0, 128)));
        let buf = rasterize_cpu(&s, 40, 40, 2.0);
        // 10 logical units at scale 2 = 20 physical px.
        assert_eq!(px(&buf, 40, 15, 15), [128, 0, 0, 128], "premultiplied half red");
        assert_eq!(px(&buf, 40, 25, 25), [0, 0, 0, 0]);
    }

    #[test]
    fn eraser_cuts_only_under_its_shape() {
        let mut s = CanvasScene::new();
        s.path().add_path(Path::rect(0.0, 0.0, 32.0, 32.0));
        s.fill(Paint::solid(Color::new(0, 0, 255, 255)));
        s.path().add_path(Path::rect(8.0, 8.0, 8.0, 8.0));
        s.fill(Paint::eraser());
        let buf = rasterize_cpu(&s, 32, 32, 1.0);
        assert_eq!(px(&buf, 32, 12, 12)[3], 0, "erased");
        assert_eq!(px(&buf, 32, 2, 2), [0, 0, 255, 255], "outside the eraser untouched");
    }

    #[test]
    fn save_restore_scopes_clips_and_transforms() {
        let mut s = CanvasScene::new();
        s.save();
        s.path().add_path(Path::rect(0.0, 0.0, 8.0, 8.0));
        s.clip();
        s.translate(4.0, 4.0);
        s.path().add_path(Path::rect(-4.0, -4.0, 32.0, 32.0));
        s.fill(Paint::solid(Color::new(0, 255, 0, 255)));
        s.restore();
        s.path().add_path(Path::rect(16.0, 16.0, 4.0, 4.0));
        s.fill(Paint::solid(Color::new(255, 255, 255, 255)));
        let buf = rasterize_cpu(&s, 32, 32, 1.0);
        assert_eq!(px(&buf, 32, 2, 2), [0, 255, 0, 255], "inside the clip");
        assert_eq!(px(&buf, 32, 12, 12)[3], 0, "clipped away");
        assert_eq!(px(&buf, 32, 17, 17), [255, 255, 255, 255], "after restore: unclipped, untranslated");
    }
}
