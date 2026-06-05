//! Web (`target_arch = "wasm32"`) renderer for the canvas SDK.
//!
//! Creates a `<canvas>` per mount and replays the author's [`Scene`]
//! into its `2d` context. Two triggers drive a repaint:
//!
//! - A reactive [`Effect`] re-runs the painter whenever a `Signal` it
//!   reads changes (same reactive-source convention as `video`/`svg`).
//! - A `ResizeObserver` re-renders (and resizes the backing store to the
//!   CSS box × `devicePixelRatio`) when layout changes the canvas size.
//!
//! Both render from one shared latest-[`Scene`] cell, so a resize never
//! re-runs author code and a content change never needs the element's
//! size to have changed.
//!
//! [`Scene`]: canvas_core::Scene

use backend_web::WebBackend;
use canvas_core::{
    paint_scene, CanvasProps, Color, DrawOp, FillRule, LineCap, LineJoin, LinearGradient, Paint,
    PaintKind, Path, PathSeg, RadialGradient, Scene, TextureLayer,
};
use runtime_core::effect;
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{
    CanvasGradient, CanvasRenderingContext2d, CanvasWindingRule, Document, HtmlCanvasElement,
    HtmlVideoElement, MediaStream, ResizeObserver,
};

/// Register the native canvas renderer against a `WebBackend`. One line
/// from the app's bootstrap; backs every `canvas::Canvas` with a
/// `<canvas>` 2D context.
pub fn register(backend: &mut WebBackend) {
    canvas_core::ensure_wire_serde();
    backend.register_external::<CanvasProps, _>(|props, _backend| build_canvas(props));
}

// Self-register at backend construction (no app-side `register` call needed).
// See [[project_inventory_self_registration]].
inventory::submit! {
    backend_web::WebExternalRegistrar(register)
}

/// Disconnects the `ResizeObserver` and frees its `Closure` on scope
/// teardown, so a callback the browser has already queued can't fire
/// into freed wasm state after unmount (the classic web-listener UAF).
struct ObserverGuard {
    observer: ResizeObserver,
    _cb: Closure<dyn FnMut()>,
}

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        self.observer.disconnect();
    }
}

fn build_canvas(props: &Rc<CanvasProps>) -> web_sys::Element {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");
    let el = document
        .create_element("canvas")
        .expect("create_element(canvas) failed");
    let _ = el.set_attribute("data-external-kind", "canvas_core::CanvasProps");

    let canvas: HtmlCanvasElement = el.clone().dyn_into().expect("canvas element cast");
    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .expect("2d context unavailable")
        .dyn_into()
        .expect("2d context cast");

    // Latest painted scene — written by the content effect, read by both
    // the effect's own render and the resize observer.
    let cell: Rc<RefCell<Scene>> = Rc::new(RefCell::new(Scene::new()));

    // Texture layers (camera): a hidden <video> per layer, drawImage'd over the
    // scene so `captureStream` records it too (web parity for camera-in-canvas).
    // Persists across renders.
    let layers = props.layers.clone();
    let layer_videos: Rc<RefCell<Vec<LayerVideo>>> = Rc::new(RefCell::new(Vec::new()));

    // Size the backing store to the CSS box × dpr, then replay the cell.
    let render: Rc<dyn Fn()> = {
        let canvas = canvas.clone();
        let ctx = ctx.clone();
        let cell = cell.clone();
        let layers = layers.clone();
        let layer_videos = layer_videos.clone();
        let document = document.clone();
        Rc::new(move || {
            render_scene(&canvas, &ctx, &cell.borrow());
            if !layers.is_empty() {
                draw_layers(&document, &ctx, &layers, &layer_videos);
            }
        })
    };

    let cb = Closure::<dyn FnMut()>::new({
        let render = render.clone();
        move || render()
    });
    let observer = ResizeObserver::new(cb.as_ref().unchecked_ref()).expect("ResizeObserver::new");
    observer.observe(&el);
    let guard = ObserverGuard { observer, _cb: cb };

    // Self-capture (web): when the canvas has a `capture` sink, hand the browser
    // the `<canvas>` via `captureStream()` and publish that `MediaStream` as the
    // stream's native source. The recorder records it directly — no readback.
    // (The app must keep the canvas re-rendering, e.g. a `version` raf, while
    // recording, or the captured stream is a frozen frame.)
    if let Some(capture) = &props.capture {
        if let Ok(stream) = canvas.capture_stream_with_frame_request_rate(CAPTURE_FPS) {
            capture.publish_native_source(Rc::new(stream));
        }
    }

    // Reactive repaint. The walker runs us inside the mount scope, so this
    // Effect (and the `guard` + `render` it owns) live until unmount.
    let props = props.clone();
    effect!({
        // Capture the observer guard into the scope-owned effect so it is
        // dropped (→ disconnected) exactly when the canvas unmounts.
        let _keep = &guard;
        *cell.borrow_mut() = paint_scene(&props);
        render();
    });

    el
}

/// Frame rate for the web self-capture `captureStream()`.
const CAPTURE_FPS: f64 = 30.0;

/// A hidden `<video>` element playing one layer's stream, reused across frames
/// (creating + attaching a stream per frame would stutter).
struct LayerVideo {
    el: HtmlVideoElement,
    /// The web `MediaStream.id` currently attached — only re-`set_src_object`
    /// when it changes (camera opened / swapped).
    stream_id: Option<String>,
}

impl LayerVideo {
    fn new(document: &Document) -> Self {
        let el: HtmlVideoElement = document
            .create_element("video")
            .expect("create_element(video)")
            .dyn_into()
            .expect("video element cast");
        // Muted + autoplay so a detached element plays without user gesture;
        // playsinline avoids iOS Safari fullscreen takeover.
        el.set_muted(true);
        el.set_autoplay(true);
        let _ = el.set_attribute("playsinline", "");
        Self { el, stream_id: None }
    }

    fn ensure(&mut self, ms: &MediaStream) {
        let id = ms.id();
        if self.stream_id.as_deref() != Some(id.as_str()) {
            self.el.set_src_object(Some(ms));
            let _ = self.el.play(); // Promise; ignore
            self.stream_id = Some(id);
        }
    }
}

/// Draw each layer's stream over the scene. The ctx already carries the dpr base
/// transform (set in `render_scene`), so we work in LOGICAL coordinates — same
/// space as the rect. Cover-fit (centered crop) + rounded-rect clip + opacity,
/// matching the macOS GPU `LayerCompositor`.
fn draw_layers(
    document: &Document,
    ctx: &CanvasRenderingContext2d,
    layers: &[TextureLayer],
    videos: &Rc<RefCell<Vec<LayerVideo>>>,
) {
    let mut vids = videos.borrow_mut();
    for (i, layer) in layers.iter().enumerate() {
        let Some(stream) = (layer.source)() else { continue };
        let Some(ms) = stream
            .native_source()
            .and_then(|rc| rc.downcast::<MediaStream>().ok())
        else {
            continue;
        };
        while vids.len() <= i {
            vids.push(LayerVideo::new(document));
        }
        let lv = &mut vids[i];
        lv.ensure(&ms);
        let (vw, vh) = (lv.el.video_width() as f32, lv.el.video_height() as f32);
        if vw < 1.0 || vh < 1.0 {
            continue; // first frames not decoded yet
        }
        let (dx, dy, dw, dh) = (layer.rect)();
        if dw < 1.0 || dh < 1.0 {
            continue;
        }
        // Shared crop/letterbox math (same on every backend).
        let ((sx, sy, sw, sh), (ox, oy, ow, oh)) =
            layer.fit.map_rects(vw, vh, dx, dy, dw, dh);
        // Clip to the DRAWN rect (letterboxed for Contain) so corners round the
        // image, not the empty bars.
        let r = (layer.corner_radius as f64).clamp(0.0, (ow.min(oh) as f64) * 0.5);

        ctx.save();
        ctx.set_global_alpha(layer.opacity.clamp(0.0, 1.0) as f64);
        ctx.begin_path();
        let _ = ctx.round_rect_with_f64(ox as f64, oy as f64, ow as f64, oh as f64, r);
        ctx.clip();
        let _ = ctx.draw_image_with_html_video_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
            &lv.el, sx as f64, sy as f64, sw as f64, sh as f64, ox as f64, oy as f64, ow as f64,
            oh as f64,
        );
        // Border frame, composited WITH the image (stays locked to the moving
        // picture). Stroked on a rounded rect inset by half the width.
        let bw = layer.border_width as f64;
        if bw > 0.0 {
            let inset = bw * 0.5;
            let br = (r - inset).max(0.0);
            ctx.begin_path();
            let _ = ctx.round_rect_with_f64(
                ox as f64 + inset,
                oy as f64 + inset,
                ow as f64 - bw,
                oh as f64 - bw,
                br,
            );
            ctx.set_line_width(bw);
            ctx.set_stroke_style_str(&rgba_css(layer.border_color));
            ctx.stroke();
        }
        ctx.restore();
    }
}

/// Resize the backing store and replay `scene` into `ctx`.
fn render_scene(canvas: &HtmlCanvasElement, ctx: &CanvasRenderingContext2d, scene: &Scene) {
    let dpr = web_sys::window().map(|w| w.device_pixel_ratio()).unwrap_or(1.0);
    let css_w = canvas.client_width() as f64;
    let css_h = canvas.client_height() as f64;
    // Not laid out yet (handler runs before insertion). The ResizeObserver
    // fires once the element has a real box and we render then.
    if css_w <= 0.0 || css_h <= 0.0 {
        return;
    }
    let bw = (css_w * dpr).round().max(1.0) as u32;
    let bh = (css_h * dpr).round().max(1.0) as u32;
    // Setting width/height resets the context; only do it on a real change.
    if canvas.width() != bw {
        canvas.set_width(bw);
    }
    if canvas.height() != bh {
        canvas.set_height(bh);
    }

    let _ = ctx.reset_transform();
    ctx.clear_rect(0.0, 0.0, bw as f64, bh as f64);
    // Author coordinates are logical pixels; map them to device pixels.
    let _ = ctx.scale(dpr, dpr);
    // Protect the dpr base transform from an unbalanced author `restore`.
    ctx.save();
    for op in scene.ops() {
        apply_op(ctx, op);
    }
    ctx.restore();
}

fn apply_op(ctx: &CanvasRenderingContext2d, op: &DrawOp) {
    match op {
        DrawOp::Save => ctx.save(),
        DrawOp::Restore => ctx.restore(),
        DrawOp::Transform(t) => {
            let _ = ctx.transform(
                t.a as f64,
                t.b as f64,
                t.c as f64,
                t.d as f64,
                t.e as f64,
                t.f as f64,
            );
        }
        DrawOp::Fill { path, paint, fill_rule } => {
            build_path(ctx, path);
            apply_fill_paint(ctx, paint);
            match fill_rule {
                FillRule::NonZero => ctx.fill(),
                FillRule::EvenOdd => ctx.fill_with_canvas_winding_rule(CanvasWindingRule::Evenodd),
            }
        }
        DrawOp::Stroke { path, paint, stroke } => {
            build_path(ctx, path);
            apply_stroke_paint(ctx, paint);
            ctx.set_line_width(stroke.width as f64);
            ctx.set_line_cap(match stroke.cap {
                LineCap::Butt => "butt",
                LineCap::Round => "round",
                LineCap::Square => "square",
            });
            ctx.set_line_join(match stroke.join {
                LineJoin::Miter => "miter",
                LineJoin::Round => "round",
                LineJoin::Bevel => "bevel",
            });
            ctx.set_miter_limit(stroke.miter_limit as f64);
            ctx.stroke();
        }
        DrawOp::Clip { path, fill_rule } => {
            build_path(ctx, path);
            ctx.clip_with_canvas_winding_rule(match fill_rule {
                FillRule::NonZero => CanvasWindingRule::Nonzero,
                FillRule::EvenOdd => CanvasWindingRule::Evenodd,
            });
        }
        // `DrawOp` is `#[non_exhaustive]`; future ops no-op until wired.
        _ => {}
    }
}

fn build_path(ctx: &CanvasRenderingContext2d, path: &Path) {
    ctx.begin_path();
    for seg in &path.segs {
        match seg {
            PathSeg::MoveTo { x, y } => ctx.move_to(*x as f64, *y as f64),
            PathSeg::LineTo { x, y } => ctx.line_to(*x as f64, *y as f64),
            PathSeg::QuadTo { cx, cy, x, y } => {
                ctx.quadratic_curve_to(*cx as f64, *cy as f64, *x as f64, *y as f64)
            }
            PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y } => ctx.bezier_curve_to(
                *c1x as f64,
                *c1y as f64,
                *c2x as f64,
                *c2y as f64,
                *x as f64,
                *y as f64,
            ),
            PathSeg::Close => ctx.close_path(),
        }
    }
}

fn apply_fill_paint(ctx: &CanvasRenderingContext2d, paint: &Paint) {
    match &paint.kind {
        PaintKind::Solid(c) => ctx.set_fill_style_str(&rgba_css(*c)),
        PaintKind::Linear(g) => ctx.set_fill_style_canvas_gradient(&linear_gradient(ctx, g)),
        PaintKind::Radial(g) => {
            if let Some(grad) = radial_gradient(ctx, g) {
                ctx.set_fill_style_canvas_gradient(&grad);
            }
        }
        // `PaintKind` is `#[non_exhaustive]`; unknown paints draw nothing.
        _ => ctx.set_fill_style_str("rgba(0,0,0,0)"),
    }
}

fn apply_stroke_paint(ctx: &CanvasRenderingContext2d, paint: &Paint) {
    match &paint.kind {
        PaintKind::Solid(c) => ctx.set_stroke_style_str(&rgba_css(*c)),
        PaintKind::Linear(g) => ctx.set_stroke_style_canvas_gradient(&linear_gradient(ctx, g)),
        PaintKind::Radial(g) => {
            if let Some(grad) = radial_gradient(ctx, g) {
                ctx.set_stroke_style_canvas_gradient(&grad);
            }
        }
        _ => ctx.set_stroke_style_str("rgba(0,0,0,0)"),
    }
}

fn linear_gradient(ctx: &CanvasRenderingContext2d, g: &LinearGradient) -> CanvasGradient {
    let grad =
        ctx.create_linear_gradient(g.x0 as f64, g.y0 as f64, g.x1 as f64, g.y1 as f64);
    for s in &g.stops {
        let _ = grad.add_color_stop(s.offset, &rgba_css(s.color));
    }
    grad
}

fn radial_gradient(ctx: &CanvasRenderingContext2d, g: &RadialGradient) -> Option<CanvasGradient> {
    let grad = ctx
        .create_radial_gradient(g.cx as f64, g.cy as f64, 0.0, g.cx as f64, g.cy as f64, g.r as f64)
        .ok()?;
    for s in &g.stops {
        let _ = grad.add_color_stop(s.offset, &rgba_css(s.color));
    }
    Some(grad)
}

/// `Rgba` → CSS `rgba(r,g,b,a)` with alpha in `0..=1`.
fn rgba_css(c: Color) -> String {
    format!("rgba({},{},{},{})", c.r, c.g, c.b, c.a as f32 / 255.0)
}
