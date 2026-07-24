//! Linux/GTK4 renderer for the canvas SDK — native 2D via Cairo.
//!
//! The GTK sibling of [`crate::macos`]. A `GtkWidget` subclass
//! ([`IdealystCanvas`]) holds the current [`Scene`](canvas_core::Scene)
//! and replays its [`DrawOp`]s into the `cairo::Context` obtained from
//! `gtk::Snapshot::append_cairo` during `snapshot()`.
//!
//! Cairo is the right engine here for the same reason CoreGraphics is on
//! Apple and Canvas2D is on web: it is the platform's own 2D engine, GTK
//! already links it, and it needs no GPU — so the canvas works wherever
//! the app does, including over remote/software rendering where the GL
//! path would not.
//!
//! # Coordinate space
//!
//! Cairo's user space starts top-left with y growing down, matching the
//! canvas `Scene`'s logical coordinates exactly. No axis flip is applied
//! (same as iOS/macOS). If strokes ever render upside-down, that is the
//! thing to check.
//!
//! # Layers
//!
//! [`DrawOp::Layer`] and [`DrawOp::LayerCached`] are *persistent* raster
//! surfaces that survive across frames — that is what lets a
//! 10k-stroke drawing avoid replaying 10k ops per frame, and what makes
//! `BlendMode::DestinationOut` a true pixel eraser rather than a
//! within-frame one. Here they are `cairo::ImageSurface`s cached by id
//! on the widget, sized to the drawable and rebuilt on resize. Mechanism
//! differs from the GPU backend; observable pixels do not (CLAUDE.md §7).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use backend_linux::gtk4;
use canvas_core::{
    BlendMode, CanvasProps, Color, DrawOp, FillRule, LineCap, LineJoin, Paint, PaintKind, Path,
    PathSeg, Rect, Scene, Stroke, Transform,
};
use gtk4::cairo;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use runtime_core::effect;

// ============================================================================
// Op replay
// ============================================================================

/// Cairo's `ImageSurface` stride is in bytes and may exceed `w * 4`.
const BPP: usize = 4;

fn set_color(ctx: &cairo::Context, c: Color) {
    ctx.set_source_rgba(
        c.r as f64 / 255.0,
        c.g as f64 / 255.0,
        c.b as f64 / 255.0,
        c.a as f64 / 255.0,
    );
}

fn operator(blend: BlendMode) -> cairo::Operator {
    match blend {
        BlendMode::Normal => cairo::Operator::Over,
        // The eraser: source COVERAGE removes destination, colour ignored.
        BlendMode::DestinationOut => cairo::Operator::DestOut,
        BlendMode::Multiply => cairo::Operator::Multiply,
        BlendMode::Screen => cairo::Operator::Screen,
        BlendMode::Overlay => cairo::Operator::Overlay,
        BlendMode::Darken => cairo::Operator::Darken,
        BlendMode::Lighten => cairo::Operator::Lighten,
        // `DrawOp` and `BlendMode` are `#[non_exhaustive]`; an unknown
        // mode degrades to source-over rather than dropping the draw.
        _ => cairo::Operator::Over,
    }
}

fn fill_rule(rule: FillRule) -> cairo::FillRule {
    match rule {
        FillRule::EvenOdd => cairo::FillRule::EvenOdd,
        _ => cairo::FillRule::Winding,
    }
}

fn line_cap(cap: LineCap) -> cairo::LineCap {
    match cap {
        LineCap::Round => cairo::LineCap::Round,
        LineCap::Square => cairo::LineCap::Square,
        _ => cairo::LineCap::Butt,
    }
}

fn line_join(join: LineJoin) -> cairo::LineJoin {
    match join {
        LineJoin::Round => cairo::LineJoin::Round,
        LineJoin::Bevel => cairo::LineJoin::Bevel,
        _ => cairo::LineJoin::Miter,
    }
}

/// Lay `path` into the context's current path.
///
/// Cairo has no quadratic primitive, so `QuadTo` is elevated to the
/// equivalent cubic (control points at 1/3 and 2/3 along the legs) —
/// exact, not an approximation.
fn build_path(ctx: &cairo::Context, path: &Path) {
    ctx.new_path();
    // Cairo needs a current point for `curve_to`/`line_to`; a path that
    // opens with geometry rather than a MoveTo would otherwise error.
    let mut cur = (0.0f64, 0.0f64);
    let mut have_current = false;
    for seg in &path.segs {
        match *seg {
            PathSeg::MoveTo { x, y } => {
                cur = (x as f64, y as f64);
                have_current = true;
                ctx.move_to(cur.0, cur.1);
            }
            PathSeg::LineTo { x, y } => {
                if !have_current {
                    ctx.move_to(x as f64, y as f64);
                    have_current = true;
                }
                cur = (x as f64, y as f64);
                ctx.line_to(cur.0, cur.1);
            }
            PathSeg::QuadTo { cx, cy, x, y } => {
                if !have_current {
                    ctx.move_to(cx as f64, cy as f64);
                    cur = (cx as f64, cy as f64);
                    have_current = true;
                }
                let (x0, y0) = cur;
                let (cx, cy) = (cx as f64, cy as f64);
                let (x1, y1) = (x as f64, y as f64);
                // Quadratic → cubic degree elevation.
                let c1 = (x0 + 2.0 / 3.0 * (cx - x0), y0 + 2.0 / 3.0 * (cy - y0));
                let c2 = (x1 + 2.0 / 3.0 * (cx - x1), y1 + 2.0 / 3.0 * (cy - y1));
                ctx.curve_to(c1.0, c1.1, c2.0, c2.1, x1, y1);
                cur = (x1, y1);
            }
            PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y } => {
                if !have_current {
                    ctx.move_to(c1x as f64, c1y as f64);
                    cur = (c1x as f64, c1y as f64);
                    have_current = true;
                }
                ctx.curve_to(
                    c1x as f64, c1y as f64, c2x as f64, c2y as f64, x as f64, y as f64,
                );
                cur = (x as f64, y as f64);
            }
            PathSeg::Close => ctx.close_path(),
            // `PathSeg` is `#[non_exhaustive]`.
            _ => {}
        }
    }
}

/// Install `paint` as the context's source.
fn set_paint(ctx: &cairo::Context, paint: &Paint) {
    match &paint.kind {
        PaintKind::Solid(c) => set_color(ctx, *c),
        PaintKind::Linear(g) => {
            let grad =
                cairo::LinearGradient::new(g.x0 as f64, g.y0 as f64, g.x1 as f64, g.y1 as f64);
            for stop in &g.stops {
                let c = stop.color;
                grad.add_color_stop_rgba(
                    stop.offset as f64,
                    c.r as f64 / 255.0,
                    c.g as f64 / 255.0,
                    c.b as f64 / 255.0,
                    c.a as f64 / 255.0,
                );
            }
            let _ = ctx.set_source(&grad);
        }
        PaintKind::Radial(g) => {
            // Canvas radial gradients run from a zero-radius focus at the
            // centre out to `r`, matching Canvas2D's two-circle form.
            let grad = cairo::RadialGradient::new(
                g.cx as f64,
                g.cy as f64,
                0.0,
                g.cx as f64,
                g.cy as f64,
                g.r as f64,
            );
            for stop in &g.stops {
                let c = stop.color;
                grad.add_color_stop_rgba(
                    stop.offset as f64,
                    c.r as f64 / 255.0,
                    c.g as f64 / 255.0,
                    c.b as f64 / 255.0,
                    c.a as f64 / 255.0,
                );
            }
            let _ = ctx.set_source(&grad);
        }
        _ => set_color(ctx, Color::BLACK),
    }
}

fn apply_stroke_style(ctx: &cairo::Context, s: &Stroke) {
    ctx.set_line_width(s.width.max(0.0) as f64);
    ctx.set_line_cap(line_cap(s.cap));
    ctx.set_line_join(line_join(s.join));
    ctx.set_miter_limit(s.miter_limit.max(1.0) as f64);
    if s.dash.is_empty() {
        ctx.set_dash(&[], 0.0);
    } else {
        let dashes: Vec<f64> = s.dash.iter().map(|d| (*d).max(0.0) as f64).collect();
        ctx.set_dash(&dashes, s.dash_offset as f64);
    }
}

fn cairo_matrix(t: &Transform) -> cairo::Matrix {
    cairo::Matrix::new(
        t.a as f64, t.b as f64, t.c as f64, t.d as f64, t.e as f64, t.f as f64,
    )
}

/// Straight RGBA8 (the scene's format) → Cairo `ARgb32`, which is
/// **premultiplied BGRA** in native-endian u32 order. Both conversions
/// are required; skipping the premultiply makes every semi-transparent
/// image render too bright, and skipping the swap turns red into blue.
fn rgba_to_cairo_surface(
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Option<cairo::ImageSurface> {
    let (w, h) = (width as i32, height as i32);
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
    let stride = surface.stride() as usize;
    {
        let mut data = surface.data().ok()?;
        for y in 0..height as usize {
            let src_row = y * width as usize * BPP;
            let dst_row = y * stride;
            for x in 0..width as usize {
                let s = src_row + x * BPP;
                if s + 3 >= rgba.len() {
                    break;
                }
                let (r, g, b, a) = (rgba[s], rgba[s + 1], rgba[s + 2], rgba[s + 3]);
                let pm = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
                let d = dst_row + x * BPP;
                // Native-endian u32 0xAARRGGBB ⇒ little-endian byte order
                // B, G, R, A.
                data[d] = pm(b);
                data[d + 1] = pm(g);
                data[d + 2] = pm(r);
                data[d + 3] = a;
            }
        }
    }
    Some(surface)
}

/// Persistent per-layer rasters, keyed by `DrawOp::Layer`/`LayerCached` id.
#[derive(Default)]
struct LayerCache {
    surfaces: HashMap<u32, cairo::ImageSurface>,
    /// Drawable size the cached surfaces were built for; a change
    /// invalidates them all (a stale-size raster would composite wrong).
    size: (i32, i32),
}

impl LayerCache {
    fn surface_for(&mut self, id: u32, w: i32, h: i32) -> Option<cairo::ImageSurface> {
        if self.size != (w, h) {
            self.surfaces.clear();
            self.size = (w, h);
        }
        if let Some(s) = self.surfaces.get(&id) {
            return Some(s.clone());
        }
        let s = cairo::ImageSurface::create(cairo::Format::ARgb32, w.max(1), h.max(1)).ok()?;
        self.surfaces.insert(id, s.clone());
        Some(s)
    }
}

/// Replay `ops` into `ctx`.
fn replay(ctx: &cairo::Context, ops: &[DrawOp], layers: &mut LayerCache, size: (i32, i32)) {
    for op in ops {
        match op {
            DrawOp::Fill { path, paint, fill_rule: rule } => {
                let _ = ctx.save();
                ctx.set_operator(operator(paint.blend));
                ctx.set_fill_rule(fill_rule(*rule));
                build_path(ctx, path);
                set_paint(ctx, paint);
                let _ = ctx.fill();
                let _ = ctx.restore();
            }
            DrawOp::Stroke { path, paint, stroke } => {
                let _ = ctx.save();
                ctx.set_operator(operator(paint.blend));
                build_path(ctx, path);
                set_paint(ctx, paint);
                apply_stroke_style(ctx, stroke);
                let _ = ctx.stroke();
                let _ = ctx.restore();
            }
            DrawOp::Save => {
                let _ = ctx.save();
            }
            DrawOp::Restore => {
                let _ = ctx.restore();
            }
            DrawOp::Transform(t) => ctx.transform(cairo_matrix(t)),
            DrawOp::Clip { path, fill_rule: rule } => {
                ctx.set_fill_rule(fill_rule(*rule));
                build_path(ctx, path);
                ctx.clip();
            }
            DrawOp::Image { image, dst, alpha, blend } => {
                draw_image(ctx, image, dst, *alpha, *blend);
            }
            DrawOp::Layer { id, clear, ops, alpha, blend } => {
                let Some(surface) = layers.surface_for(*id, size.0, size.1) else {
                    continue;
                };
                if let Ok(lctx) = cairo::Context::new(&surface) {
                    if *clear {
                        lctx.set_operator(cairo::Operator::Clear);
                        let _ = lctx.paint();
                        lctx.set_operator(cairo::Operator::Over);
                    }
                    // A nested Layer would re-enter the cache we hold
                    // mutably; give the nested replay its own cache. Nested
                    // persistent layers are not part of the model.
                    let mut nested = LayerCache::default();
                    replay(&lctx, ops, &mut nested, size);
                }
                composite_surface(ctx, &surface, *alpha, *blend, None);
            }
            DrawOp::LayerCached { id, dirty, ops, transform, alpha, blend } => {
                let Some(surface) = layers.surface_for(*id, size.0, size.1) else {
                    continue;
                };
                if *dirty {
                    if let Ok(lctx) = cairo::Context::new(&surface) {
                        // A dirty bake is a full repaint of the layer over a
                        // transparent base — not an accumulation.
                        lctx.set_operator(cairo::Operator::Clear);
                        let _ = lctx.paint();
                        lctx.set_operator(cairo::Operator::Over);
                        let mut nested = LayerCache::default();
                        replay(&lctx, ops, &mut nested, size);
                    }
                }
                composite_surface(ctx, &surface, *alpha, *blend, Some(transform));
            }
            DrawOp::Shapes { shapes, blend } => {
                // Cairo has no instanced fast path: expand the batch to
                // per-shape fills, in array order, replaying each through the
                // Fill arm so a batched shape and a hand-authored fill produce
                // identical pixels (CLAUDE.md §7).
                for sh in shapes {
                    replay(ctx, std::slice::from_ref(&sh.to_fill_op(*blend)), layers, size);
                }
            }
            DrawOp::Glyphs { font, glyphs, paint } => {
                // Cairo has no embedded-font glyph engine reachable here, so —
                // like the CoreGraphics / Canvas2D / android backends — outline
                // each glyph from the run's font bytes with skrifa (at upem
                // 1000) and fill it through the existing Fill path. Both this
                // and the GPU (vello) path outline at the SAME upem with hinting
                // off, so the pixels match (CLAUDE.md §7). `expand_run` returns
                // Save·Transform·Fill·Restore quartets; replaying them reuses the
                // Fill arm's colour/alpha/blend handling verbatim, and the
                // per-glyph Transform composes on top of the context's current
                // (accumulated) CTM exactly as the run's affine intends.
                let ops = crate::glyphs::expand_run(font, glyphs, paint);
                replay(ctx, &ops, layers, size);
            }
            // `DrawOp` is `#[non_exhaustive]`; any future op without a Cairo
            // mapping no-ops rather than abort the frame.
            _ => {}
        }
    }
}

/// Composite a baked layer surface into `ctx`, optionally under a
/// logical-coordinate affine.
fn composite_surface(
    ctx: &cairo::Context,
    surface: &cairo::ImageSurface,
    alpha: f32,
    blend: BlendMode,
    transform: Option<&Transform>,
) {
    let _ = ctx.save();
    ctx.set_operator(operator(blend));
    if let Some(t) = transform {
        ctx.transform(cairo_matrix(t));
    }
    let _ = ctx.set_source_surface(surface, 0.0, 0.0);
    let _ = ctx.paint_with_alpha(alpha.clamp(0.0, 1.0) as f64);
    let _ = ctx.restore();
}

fn draw_image(
    ctx: &cairo::Context,
    image: &canvas_core::ImageSource,
    dst: &Rect,
    alpha: f32,
    blend: BlendMode,
) {
    let Some(surface) = rgba_to_cairo_surface(image.width, image.height, &image.rgba) else {
        return;
    };
    if image.width == 0 || image.height == 0 || dst.w <= 0.0 || dst.h <= 0.0 {
        return;
    }
    let _ = ctx.save();
    ctx.set_operator(operator(blend));
    // Scale the source to fill `dst`, then clip so a non-uniform scale
    // can't bleed past the destination rect.
    ctx.translate(dst.x as f64, dst.y as f64);
    ctx.scale(
        dst.w as f64 / image.width as f64,
        dst.h as f64 / image.height as f64,
    );
    let _ = ctx.set_source_surface(&surface, 0.0, 0.0);
    ctx.rectangle(0.0, 0.0, image.width as f64, image.height as f64);
    let _ = ctx.clip();
    let _ = ctx.paint_with_alpha(alpha.clamp(0.0, 1.0) as f64);
    let _ = ctx.restore();
}

// ============================================================================
// Widget
// ============================================================================

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct IdealystCanvas {
        pub scene: RefCell<Scene>,
        pub layers: RefCell<LayerCache>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for IdealystCanvas {
        const NAME: &'static str = "IdealystCanvas";
        type Type = super::IdealystCanvas;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for IdealystCanvas {}

    impl WidgetImpl for IdealystCanvas {
        fn snapshot(&self, snapshot: &gtk4::Snapshot) {
            let obj = self.obj();
            let (w, h) = (obj.width(), obj.height());
            if w <= 0 || h <= 0 {
                return;
            }
            // `append_cairo` hands back a Cairo context whose user space is
            // the widget's own top-left-origin logical coordinates — the
            // canvas Scene's space exactly, so no flip or offset is needed.
            let ctx = snapshot.append_cairo(&gtk4::graphene::Rect::new(
                0.0,
                0.0,
                w as f32,
                h as f32,
            ));
            let scene = self.scene.borrow();
            let mut layers = self.layers.borrow_mut();
            replay(&ctx, scene.ops(), &mut layers, (w, h));
        }
    }
}

use gtk4::glib;

glib::wrapper! {
    /// GTK widget that replays a canvas [`Scene`](canvas_core::Scene)
    /// into Cairo.
    pub struct IdealystCanvas(ObjectSubclass<imp::IdealystCanvas>)
        @extends gtk4::Widget;
}

impl Default for IdealystCanvas {
    fn default() -> Self {
        Self::new()
    }
}

impl IdealystCanvas {
    /// Build an empty canvas widget.
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Swap the scene and queue a redraw.
    fn install_scene(&self, scene: Scene) {
        *self.imp().scene.borrow_mut() = scene;
        self.queue_draw();
    }
}

// ============================================================================
// register + build
// ============================================================================

/// Register the Linux/GTK canvas renderer against a `LinuxBackend`.
pub fn register(backend: &mut backend_linux::LinuxBackend) {
    canvas_core::ensure_wire_serde();
    backend.register_external::<CanvasProps, _>(|props, b| build_canvas(props, b));
}

// Self-register at backend construction (no app-side `register` call
// needed). Behind the default-on `self-register` feature, matching every
// other platform module in this crate.
#[cfg(feature = "self-register")]
inventory::submit! {
    backend_linux::LinuxExternalRegistrar(register)
}

fn build_canvas(
    props: &Rc<CanvasProps>,
    b: &mut backend_linux::LinuxBackend,
) -> backend_linux::LinuxNode {
    let widget = IdealystCanvas::new();
    let node = b.register_external_view(widget.clone().upcast::<gtk4::Widget>());

    let widget_for_effect = widget.clone();
    let props_clone = props.clone();
    effect!({
        let scene = canvas_core::paint_scene(&props_clone);
        widget_for_effect.install_scene(scene);
    });

    node
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    // `Color`, `Paint`, `Path`, `PathSeg`, `Transform`, `Scene` are already in
    // scope via the module's own `use canvas_core::{…}`.
    use super::*;
    use canvas_core::{FontResource, PositionedGlyph};
    use skrifa::instance::{LocationRef, Size};
    use skrifa::outline::{DrawSettings, OutlinePen};
    use skrifa::{FontRef, GlyphId, MetadataProvider};

    /// The em a glyph run is normalized to — must match `glyphs::GLYPH_UPEM`.
    const UPEM: f32 = 1000.0;

    /// A real system font's bytes + face index, or `None` to skip on a CI box
    /// with no fonts installed.
    fn load_test_font() -> Option<(Vec<u8>, u32)> {
        for path in [
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
        ] {
            if let Ok(bytes) = std::fs::read(path) {
                return Some((bytes, 0));
            }
        }
        None
    }

    /// Replay `scene` into a fresh transparent ARgb32 surface and return its
    /// premultiplied-BGRA bytes + row stride. No GTK display needed — Cairo's
    /// image surface is self-contained.
    fn render(scene: &Scene, s: i32) -> (Vec<u8>, usize) {
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, s, s).unwrap();
        {
            let ctx = cairo::Context::new(&surface).unwrap();
            let mut layers = LayerCache::default();
            replay(&ctx, scene.ops(), &mut layers, (s, s));
            // The context must be dropped before `surface.data()` — a live
            // context holds an exclusive borrow of the surface pixels.
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().unwrap().to_vec();
        (data, stride)
    }

    /// A skrifa outline pen recording into a canvas `Path` (font-design units,
    /// y-up), the same expansion `glyphs::expand_run` performs internally.
    #[derive(Default)]
    struct PathPen(Path);
    impl OutlinePen for PathPen {
        fn move_to(&mut self, x: f32, y: f32) {
            self.0.segs.push(PathSeg::MoveTo { x, y });
        }
        fn line_to(&mut self, x: f32, y: f32) {
            self.0.segs.push(PathSeg::LineTo { x, y });
        }
        fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
            self.0.segs.push(PathSeg::QuadTo { cx, cy, x, y });
        }
        fn curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
            self.0.segs.push(PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y });
        }
        fn close(&mut self) {
            self.0.segs.push(PathSeg::Close);
        }
    }

    /// Regression for the no-op `DrawOp::Glyphs` arm: a glyph run must actually
    /// rasterize onto the Cairo surface. Before this backend outlined glyphs,
    /// the arm was a no-op and canvas/PDF-export text vanished — `ink` would be
    /// 0 and this fails. The run is also compared, pixel-for-pixel, against the
    /// same glyph authored as an outline `Fill` under the same transform: the
    /// contract is that the two produce the same pixels (CLAUDE.md §7), which
    /// also catches a mirrored/mis-scaled glyph (a filled-but-wrong arm).
    #[test]
    fn regression_glyph_run_rasterizes_on_cairo() {
        const S: i32 = 64;
        let Some((bytes, index)) = load_test_font() else {
            eprintln!("skip: no system font");
            return;
        };
        let font_ref = FontRef::from_index(&bytes, index).expect("parse font");
        // 'F' — vertically asymmetric, so a flip/mirror is unmissable.
        let Some(gid) = font_ref.charmap().map('F') else {
            eprintln!("skip: font lacks 'F'");
            return;
        };
        // 48px 'F', y-up outline flipped to y-down (d < 0, the page flip a PDF
        // carries), baseline near y≈52, left edge x=12.
        let em = 48.0 / UPEM;
        let t = Transform { a: em, b: 0.0, c: 0.0, d: -em, e: 12.0, f: 52.0 };
        let black = Paint::solid(Color::new(0, 0, 0, 255));

        // (1) The glyph run through the Glyphs arm under test.
        let mut run = Scene::new();
        run.glyphs(
            FontResource::new(0xF0, index, bytes.clone()),
            [PositionedGlyph::new(gid.to_u32(), t)],
            black.clone(),
        );

        // (2) The same glyph as a hand-authored outline Fill (skrifa at upem
        // 1000, same `t`) — an independent reference for the expected pixels.
        let mut pen = PathPen::default();
        font_ref
            .outline_glyphs()
            .get(GlyphId::new(gid.to_u32()))
            .expect("outline")
            .draw(DrawSettings::unhinted(Size::new(UPEM), LocationRef::default()), &mut pen)
            .expect("draw outline");
        let mut reference = Scene::new();
        reference.save();
        reference.transform(t);
        reference.add_path(pen.0);
        reference.fill(black);
        reference.restore();

        let (run_px, stride) = render(&run, S);
        let (ref_px, _) = render(&reference, S);

        // ARgb32 is premultiplied BGRA, native-endian; the alpha byte is [3].
        let inked = |px: &[u8], x: i32, y: i32| px[y as usize * stride + x as usize * BPP + 3] > 128;
        let (mut ink_run, mut ink_ref, mut mismatch) = (0u32, 0u32, 0u32);
        for y in 0..S {
            for x in 0..S {
                let (a, b) = (inked(&run_px, x, y), inked(&ref_px, x, y));
                ink_run += a as u32;
                ink_ref += b as u32;
                mismatch += (a != b) as u32;
            }
        }
        assert!(ink_run > 40, "glyph run drew no ink ({ink_run}) — Glyphs arm is a no-op");
        assert!(ink_ref > 40, "outline reference drew no ink ({ink_ref})");
        // Correctly-oriented/scaled run overlaps the outline almost perfectly
        // (only antialiased edges differ); a flipped or mis-scaled run diverges
        // on most of its ink.
        assert!(
            mismatch < ink_ref / 3,
            "glyph run diverges from its outline (mismatch {mismatch} vs ink {ink_ref})"
        );
    }

    /// A `DrawOp::Shapes` batch must expand to per-shape fills on Cairo (the arm
    /// was also a no-op). One solid rounded-rect must paint ink where it sits.
    #[test]
    fn shapes_batch_rasterizes_on_cairo() {
        const S: i32 = 64;
        let mut scene = Scene::new();
        scene.shapes([canvas_core::ShapeInstance::rect(
            8.0,
            8.0,
            32.0,
            32.0,
            Color::new(0, 0, 0, 255),
        )]);
        let (px, stride) = render(&scene, S);
        let alpha = |x: i32, y: i32| px[y as usize * stride + x as usize * BPP + 3];
        // Centre of the rect is inked; a corner well outside it is not.
        assert!(alpha(24, 24) > 128, "shape batch drew no ink — Shapes arm is a no-op");
        assert_eq!(alpha(60, 60), 0, "shape ink leaked outside its rect");
    }
}
