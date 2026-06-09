//! Scene → vello translation, shared by the native (`render.rs`) and web
//! (`render_web.rs`) renderers.
//!
//! This is the platform- and async-agnostic core: it walks a
//! `canvas_core::Scene`'s op list into a `vello::Scene`, maintaining the
//! transform stack, clip layers, blend-mode wrapping, persistent layers, and
//! the image-upload cache. Both backends feed the resulting `vello::Scene` to a
//! `vello::Renderer`; the only thing that differs between them is how the GPU
//! device/surface is acquired (blocking on native, async on web). Keeping the
//! encoder here guarantees byte-identical output across targets (CLAUDE.md §7).

use canvas_core::{
    BlendMode as CanvasBlend, Color as CanvasColor, DrawOp, FillRule, GradientStop,
    ImageSource as CanvasImage, LineCap, LineJoin, Paint, PaintKind, Path, PathSeg,
    Stroke as CanvasStroke,
};

use std::cell::RefCell;
use std::collections::HashMap;

use vello::kurbo::{Affine, BezPath, Cap, Join, Point, Rect, Shape, Stroke as KurboStroke};
use vello::peniko::color::DynamicColor;
use vello::peniko::{
    BlendMode, Blob, Brush, Color, ColorStop, Compose, Fill, Gradient, ImageAlphaType, ImageBrush,
    ImageData, ImageFormat, Mix,
};
use vello::Scene as VelloScene;

/// Walk a canvas op list into `vs`, maintaining a transform stack
/// (Save/Restore + Transform) and clip layers (Clip → push_layer). Takes a
/// raw op slice (not a `Scene`) so persistent-layer nested ops can recurse
/// through the same encoder.
pub(crate) fn encode_scene(ops: &[DrawOp], vs: &mut VelloScene, base: Affine) {
    let mut cur = base;
    // (saved transform, number of clip layers pushed inside this save scope)
    let mut stack: Vec<(Affine, u32)> = Vec::new();
    // Clips pushed outside any save scope (popped at the end).
    let mut root_clips: u32 = 0;

    for op in ops {
        match op {
            DrawOp::Save => stack.push((cur, 0)),
            DrawOp::Restore => {
                if let Some((saved, n_clips)) = stack.pop() {
                    for _ in 0..n_clips {
                        vs.pop_layer();
                    }
                    cur = saved;
                }
            }
            DrawOp::Transform(t) => {
                cur *= affine_of(t);
            }
            DrawOp::Clip { path, fill_rule } => {
                let shape = bez_of(path);
                // A clip layer: clip to the path interior (its fill rule),
                // Normal blend, full alpha. Popped at the matching Restore.
                vs.push_layer(fill_of(*fill_rule), Mix::Normal, 1.0, cur, &shape);
                match stack.last_mut() {
                    Some(top) => top.1 += 1,
                    None => root_clips += 1,
                }
            }
            DrawOp::Fill { path, paint, fill_rule } => {
                let shape = bez_of(path);
                let brush = brush_of(paint);
                match peniko_blend(paint.blend) {
                    None => vs.fill(fill_of(*fill_rule), cur, &brush, None, &shape),
                    // Vello has no per-draw blend: wrap the fill in a layer
                    // whose pop composites it onto the backdrop with `blend`.
                    // Clip the layer to the shape's bounds so nothing outside
                    // is touched (critical for DestinationOut — it must only
                    // erase under the eraser shape).
                    Some(blend) => {
                        let bounds = shape.bounding_box();
                        vs.push_layer(Fill::NonZero, blend, 1.0, cur, &bounds);
                        vs.fill(fill_of(*fill_rule), cur, &brush, None, &shape);
                        vs.pop_layer();
                    }
                }
            }
            DrawOp::Stroke { path, paint, stroke } => {
                let shape = bez_of(path);
                let brush = brush_of(paint);
                match peniko_blend(paint.blend) {
                    None => vs.stroke(&kurbo_stroke(stroke), cur, &brush, None, &shape),
                    Some(blend) => {
                        // Inflate the clip bounds by the stroke half-width (plus
                        // a small margin) so the stroked outline isn't clipped.
                        let m = (stroke.width as f64) * 0.5 + 1.0;
                        let bounds = shape.bounding_box().inflate(m, m);
                        vs.push_layer(Fill::NonZero, blend, 1.0, cur, &bounds);
                        vs.stroke(&kurbo_stroke(stroke), cur, &brush, None, &shape);
                        vs.pop_layer();
                    }
                }
            }
            DrawOp::Image { image, dst, alpha, blend } => {
                if !image.is_valid() || image.width == 0 || image.height == 0 {
                    continue;
                }
                let data = image_data_cached(image);
                // Map the image's natural [0,0,w,h] space onto `dst`, under the
                // current transform. Non-uniform scale stretches to fit.
                let t = cur
                    * Affine::translate((dst.x as f64, dst.y as f64))
                    * Affine::scale_non_uniform(
                        dst.w as f64 / image.width as f64,
                        dst.h as f64 / image.height as f64,
                    );
                let brush = ImageBrush::new(data).with_alpha(*alpha);
                match peniko_blend(*blend) {
                    None => vs.draw_image(&brush, t),
                    Some(b) => {
                        // Clip the blend layer to the destination rect (in `cur`
                        // space) so only `dst` participates in the composite.
                        let clip = Rect::new(
                            dst.x as f64,
                            dst.y as f64,
                            (dst.x + dst.w) as f64,
                            (dst.y + dst.h) as f64,
                        );
                        vs.push_layer(Fill::NonZero, b, 1.0, cur, &clip);
                        vs.draw_image(&brush, t);
                        vs.pop_layer();
                    }
                }
            }
            DrawOp::Layer { id, clear, ops: nested, alpha, blend } => {
                // Persistent layer = a retained vello op-log kept across frames.
                // We append this frame's `nested` ops to it (or reset on
                // `clear`), then composite the whole retained log into `vs`.
                // The mechanism differs from the CPU backends' raster bake, but
                // the output converges: accumulation and DestinationOut erase
                // both replay correctly each frame (CLAUDE.md §7).
                LAYER_SCENES.with(|m| {
                    let mut map = m.borrow_mut();
                    let layer = map.entry(*id).or_insert_with(VelloScene::new);
                    if *clear {
                        layer.reset();
                    }
                    // Encode nested ops at identity — the layer holds content in
                    // canvas-logical coords; `cur` (incl. dpr) is applied at
                    // composite time via `append`.
                    encode_scene(nested, layer, Affine::IDENTITY);

                    // Always composite the layer as an ISOLATED group, even at
                    // alpha 1 / Normal blend: an eraser (DestinationOut) inside
                    // the layer must only cut the layer's own pixels, never punch
                    // through to content drawn into `vs` before this op. The
                    // isolated layer is then laid onto `vs` with `blend`.
                    let b = peniko_blend(*blend)
                        .unwrap_or(BlendMode::new(Mix::Normal, Compose::SrcOver));
                    let clip = Rect::new(-1.0e6, -1.0e6, 1.0e6, 1.0e6);
                    vs.push_layer(Fill::NonZero, b, *alpha, cur, &clip);
                    vs.append(layer, Some(cur));
                    vs.pop_layer();
                });
            }
            DrawOp::LayerCached { id, dirty, transform, ops: nested, alpha, blend } => {
                // The shared encoder has no GPU device, so it can't bake the layer
                // to a texture — that fast path lives in the renderers (render.rs /
                // render_web.rs via `ScenePlan::Cached`). This is the CORRECT
                // device-less fallback (web Canvas2D's vello-on-web path before a
                // GPU is confirmed, headless tests, and any scene whose structure
                // the plan classifier doesn't route to the fast path): a retained
                // vello op-log per layer id, re-encoded only when `dirty`, then
                // `append`ed under `transform` every frame. It retains across
                // frames (so `dirty: false` reuses last bake) and applies the
                // transform — the same observable pixels the texture-cache path
                // produces (CLAUDE.md §7), just re-rasterized rather than cached.
                CACHED_LAYER_SCENES.with(|m| {
                    let mut map = m.borrow_mut();
                    let layer = map.entry(*id).or_insert_with(VelloScene::new);
                    if *dirty {
                        layer.reset();
                        // Bake at identity — the layer holds content in its own
                        // logical coords; `transform` (+ `cur`/dpr) is applied at
                        // composite time via `append`.
                        encode_scene(nested, layer, Affine::IDENTITY);
                    }
                    // Compose the layer under the camera transform. Isolated group
                    // (like `DrawOp::Layer`) so an eraser inside the layer can't
                    // punch through content drawn into `vs` before this op.
                    let t = cur * affine_of(transform);
                    let b = peniko_blend(*blend)
                        .unwrap_or(BlendMode::new(Mix::Normal, Compose::SrcOver));
                    let clip = Rect::new(-1.0e6, -1.0e6, 1.0e6, 1.0e6);
                    vs.push_layer(Fill::NonZero, b, *alpha, t, &clip);
                    vs.append(layer, Some(t));
                    vs.pop_layer();
                });
            }
            DrawOp::Shapes { shapes, blend } => {
                // The shared encoder has no GPU device, so no instanced fast path
                // here: expand the batch to per-shape fills, in array order, at
                // the current transform. Recursing through `encode_scene` reuses
                // the Fill arm's brush + blend-layer handling verbatim, so a
                // batched shape and a hand-authored fill encode identically
                // (CLAUDE.md §7). The render.rs GPU path intercepts PURE-shape
                // scenes before encoding and draws them instanced; this remains
                // the correct fallback for the web backend and any mixed scene.
                let fills: Vec<DrawOp> = shapes.iter().map(|sh| sh.to_fill_op(*blend)).collect();
                encode_scene(&fills, vs, cur);
            }
            _ => {}
        }
    }

    // Pop any still-open clip layers (unbalanced restore, or root clips).
    for (_, n_clips) in stack.drain(..) {
        for _ in 0..n_clips {
            vs.pop_layer();
        }
    }
    for _ in 0..root_clips {
        vs.pop_layer();
    }
}

fn bez_of(path: &Path) -> BezPath {
    let mut bp = BezPath::new();
    for seg in &path.segs {
        match seg {
            PathSeg::MoveTo { x, y } => bp.move_to(pt(*x, *y)),
            PathSeg::LineTo { x, y } => bp.line_to(pt(*x, *y)),
            PathSeg::QuadTo { cx, cy, x, y } => bp.quad_to(pt(*cx, *cy), pt(*x, *y)),
            PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y } => {
                bp.curve_to(pt(*c1x, *c1y), pt(*c2x, *c2y), pt(*x, *y))
            }
            PathSeg::Close => bp.close_path(),
        }
    }
    bp
}

fn pt(x: f32, y: f32) -> Point {
    Point::new(x as f64, y as f64)
}

fn affine_of(t: &canvas_core::Transform) -> Affine {
    // Canvas Transform (a,b,c,d,e,f) maps to kurbo's [a,b,c,d,e,f] coeffs.
    Affine::new([t.a as f64, t.b as f64, t.c as f64, t.d as f64, t.e as f64, t.f as f64])
}

thread_local! {
    /// Per-render-thread persistent layer scenes ([`DrawOp::Layer`]) keyed by
    /// layer id. Each is a retained vello op-log that accumulates across
    /// frames (`clear: false`) and is reset on `clear: true`. Grows with the
    /// total ops drawn since the last clear — the same bound as the author's
    /// own vector model would carry; documented on `DrawOp::Layer`.
    static LAYER_SCENES: RefCell<HashMap<u32, VelloScene>> = RefCell::new(HashMap::new());
}

thread_local! {
    /// Per-render-thread retained op-logs for [`DrawOp::LayerCached`] keyed by
    /// layer id — the device-less fallback's counterpart to the renderers' GPU
    /// texture cache. Re-encoded only on a `dirty` bake and `append`ed under the
    /// camera transform every frame, so a `dirty: false` pan reuses the last
    /// bake. Distinct from [`LAYER_SCENES`] so cached and accumulate layers can
    /// share an id without colliding.
    static CACHED_LAYER_SCENES: RefCell<HashMap<u32, VelloScene>> = RefCell::new(HashMap::new());
}

thread_local! {
    /// Per-render-thread cache of uploaded [`ImageData`] keyed by
    /// [`CanvasImage::id`]. `ImageData` holds a refcounted [`Blob`], so vello
    /// dedupes the GPU upload across frames as long as we hand it the *same*
    /// Blob — rebuilding it each frame would defeat that. Authors emit the
    /// same `id` every frame for a static image; we build the Blob once.
    ///
    /// Note: this keeps ONE entry per image id — it OVERWRITES the slot on a
    /// content `generation` change rather than growing. A static image bumps no
    /// generation, so its Blob is built once; an animated source (video frames
    /// under one stable id) re-uploads in place. Canvas authors use a small,
    /// stable set of image ids, so unbounded growth isn't a concern; if that
    /// changes, add an LRU keyed on frame use.
    static IMAGE_CACHE: RefCell<HashMap<u64, (u64, ImageData)>> = RefCell::new(HashMap::new());
}

/// Get-or-build the cached [`ImageData`] for a canvas image. Caller has already
/// checked `is_valid()`. Re-uploads (overwriting the id's slot) when the image's
/// `generation` changed, so a video frame pumped under one stable id animates
/// instead of serving the cached first frame.
fn image_data_cached(src: &CanvasImage) -> ImageData {
    IMAGE_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((gen, data)) = c.get(&src.id) {
            if *gen == src.generation {
                return data.clone();
            }
        }
        let data = ImageData {
            data: Blob::from(src.rgba.clone()),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: src.width,
            height: src.height,
        };
        c.insert(src.id, (src.generation, data.clone()));
        data
    })
}

/// Map the canvas [`CanvasBlend`] to a peniko [`BlendMode`], or `None` for
/// `Normal` (drawn directly, no layer). DestinationOut is a Porter-Duff
/// *compose* mode (the eraser); Multiply/Screen are separable *mix* modes
/// composited source-over.
fn peniko_blend(blend: CanvasBlend) -> Option<BlendMode> {
    match blend {
        CanvasBlend::Normal => None,
        CanvasBlend::DestinationOut => Some(BlendMode::new(Mix::Normal, Compose::DestOut)),
        CanvasBlend::Multiply => Some(BlendMode::new(Mix::Multiply, Compose::SrcOver)),
        CanvasBlend::Screen => Some(BlendMode::new(Mix::Screen, Compose::SrcOver)),
        // `#[non_exhaustive]`; unknown modes draw normally.
        _ => None,
    }
}

fn fill_of(rule: FillRule) -> Fill {
    match rule {
        FillRule::NonZero => Fill::NonZero,
        FillRule::EvenOdd => Fill::EvenOdd,
    }
}

fn color_of(c: CanvasColor) -> Color {
    Color::from_rgba8(c.r, c.g, c.b, c.a)
}

fn brush_of(paint: &Paint) -> Brush {
    match &paint.kind {
        PaintKind::Solid(c) => Brush::Solid(color_of(*c)),
        PaintKind::Linear(g) => Brush::Gradient(
            Gradient::new_linear(pt(g.x0, g.y0), pt(g.x1, g.y1))
                .with_stops(stops_of(&g.stops).as_slice()),
        ),
        PaintKind::Radial(g) => Brush::Gradient(
            Gradient::new_radial(pt(g.cx, g.cy), g.r).with_stops(stops_of(&g.stops).as_slice()),
        ),
        _ => Brush::Solid(Color::from_rgba8(0, 0, 0, 0)),
    }
}

fn stops_of(stops: &[GradientStop]) -> Vec<ColorStop> {
    stops
        .iter()
        .map(|s| ColorStop {
            offset: s.offset,
            color: DynamicColor::from_alpha_color(color_of(s.color)),
        })
        .collect()
}

fn kurbo_stroke(s: &CanvasStroke) -> KurboStroke {
    KurboStroke::new(s.width as f64)
        .with_caps(match s.cap {
            LineCap::Butt => Cap::Butt,
            LineCap::Round => Cap::Round,
            LineCap::Square => Cap::Square,
        })
        .with_join(match s.join {
            LineJoin::Miter => Join::Miter,
            LineJoin::Round => Join::Round,
            LineJoin::Bevel => Join::Bevel,
        })
        .with_miter_limit(s.miter_limit as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peniko_blend_maps_each_mode() {
        // Normal is drawn directly (no layer wrap).
        assert!(peniko_blend(CanvasBlend::Normal).is_none());
        // DestinationOut is the eraser — a Porter-Duff *compose* mode.
        let d = peniko_blend(CanvasBlend::DestinationOut).expect("blend");
        assert_eq!(d.compose, Compose::DestOut);
        assert_eq!(d.mix, Mix::Normal);
        // Multiply / Screen are separable *mix* modes, composited source-over.
        assert_eq!(peniko_blend(CanvasBlend::Multiply).expect("blend").mix, Mix::Multiply);
        assert_eq!(peniko_blend(CanvasBlend::Screen).expect("blend").mix, Mix::Screen);
    }
}
