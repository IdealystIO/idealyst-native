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
            // `DrawOp` is `#[non_exhaustive]`; ops without a Cairo mapping
            // yet (instanced Shapes, Glyphs) no-op rather than abort the
            // frame. See the module TODO.
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
