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
    paint_scene, BlendMode, CanvasProps, Color, DrawOp, FillRule, ImageSource, LineCap, LineJoin,
    LinearGradient, Paint, PaintKind, Path, PathSeg, RadialGradient, Scene, TextureLayer,
};
use runtime_core::effect;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{Clamped, JsCast};
use web_sys::{
    CanvasGradient, CanvasRenderingContext2d, CanvasWindingRule, Document, HtmlCanvasElement,
    HtmlVideoElement, ImageData, MediaStream, ResizeObserver,
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

    // Latest painted scene — written by the content effect, read by both
    // the effect's own render and the resize observer.
    let cell: Rc<RefCell<Scene>> = Rc::new(RefCell::new(Scene::new()));

    // Per-frame rasterizer (2d ctx + texture layers + captureStream). Shared
    // behind `Rc<RefCell>` because both the ResizeObserver and the reactive
    // effect drive a repaint from the latest `cell`.
    let rasterize = Rc::new(RefCell::new(make_2d_rasterizer(canvas, props)));

    let cb = Closure::<dyn FnMut()>::new({
        let rasterize = rasterize.clone();
        let cell = cell.clone();
        move || (rasterize.borrow_mut())(&cell.borrow())
    });
    let observer = ResizeObserver::new(cb.as_ref().unchecked_ref()).expect("ResizeObserver::new");
    observer.observe(&el);
    let guard = ObserverGuard { observer, _cb: cb };

    // Reactive repaint. The walker runs us inside the mount scope, so this
    // Effect (and the `guard` + `rasterize` it owns) live until unmount.
    let props = props.clone();
    effect!({
        // Capture the observer guard into the scope-owned effect so it is
        // dropped (→ disconnected) exactly when the canvas unmounts.
        let _keep = &guard;
        *cell.borrow_mut() = paint_scene(&props);
        (rasterize.borrow_mut())(&cell.borrow());
    });

    el
}

/// Build a per-frame rasterizer that replays a [`Scene`] into `canvas`'s `2d`
/// context (including texture layers and `captureStream` self-capture). The
/// returned closure resizes the backing store to the CSS box × dpr and replays
/// the scene on each call; the caller owns the repaint triggers (a reactive
/// effect, a `ResizeObserver`, or the graphics primitive's `on_resize`).
///
/// Used by canvas-native's own handler AND by `canvas-vello`'s web renderer as
/// its **WebGPU-unavailable fallback**: vello hands this the SAME `<canvas>` the
/// `graphics` primitive created — still *unclaimed*, so this `getContext("2d")`
/// is the canvas's first and only context (a `<canvas>` is permanently bound to
/// its first context type on the web). Output is identical to the native-on-web
/// path (CLAUDE.md §7).
pub fn make_2d_rasterizer(
    canvas: HtmlCanvasElement,
    props: &Rc<CanvasProps>,
) -> Box<dyn FnMut(&Scene)> {
    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .expect("2d context unavailable")
        .dyn_into()
        .expect("2d context cast");

    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");

    // Texture layers (camera): a hidden <video> per layer, drawImage'd over the
    // scene so `captureStream` records it too (web parity for camera-in-canvas).
    // Persists across renders.
    let layers = props.layers.clone();
    let layer_videos: Rc<RefCell<Vec<LayerVideo>>> = Rc::new(RefCell::new(Vec::new()));

    publish_capture_stream(&canvas, props);

    Box::new(move |scene: &Scene| {
        render_scene(&canvas, &ctx, scene);
        if !layers.is_empty() {
            draw_layers(&document, &ctx, &layers, &layer_videos);
        }
    })
}

/// Publish `canvas` as a `captureStream()` [`MediaStream`] into `props.capture`
/// (web self-capture). No-op when the canvas has no `capture` sink. The recorder
/// records the canvas directly — no readback.
///
/// Shared by the 2D rasterizer and `canvas-vello`'s web **GPU** path:
/// `captureStream` works on any canvas regardless of context type, so the vello
/// path reuses this instead of a GPU→CPU readback (the readback's blocking
/// `map`+`poll` is illegal on the wasm main thread). (The app must keep the
/// canvas re-rendering, e.g. a `version` raf, while recording, or the captured
/// stream is a frozen frame.)
pub fn publish_capture_stream(canvas: &HtmlCanvasElement, props: &CanvasProps) {
    if let Some(capture) = &props.capture {
        if let Ok(stream) = canvas.capture_stream_with_frame_request_rate(CAPTURE_FPS) {
            capture.publish_native_source(Rc::new(stream));
        }
    }
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
        let r = ((layer.corner_radius)() as f64).clamp(0.0, (ow.min(oh) as f64) * 0.5);

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
            apply_blend(ctx, paint.blend);
            match fill_rule {
                FillRule::NonZero => ctx.fill(),
                FillRule::EvenOdd => ctx.fill_with_canvas_winding_rule(CanvasWindingRule::Evenodd),
            }
            clear_blend(ctx, paint.blend);
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
            apply_blend(ctx, paint.blend);
            ctx.stroke();
            clear_blend(ctx, paint.blend);
        }
        DrawOp::Clip { path, fill_rule } => {
            build_path(ctx, path);
            ctx.clip_with_canvas_winding_rule(match fill_rule {
                FillRule::NonZero => CanvasWindingRule::Nonzero,
                FillRule::EvenOdd => CanvasWindingRule::Evenodd,
            });
        }
        DrawOp::Layer { id, clear, ops: nested, alpha, blend } => {
            draw_layer(ctx, *id, *clear, nested, *alpha, *blend);
        }
        DrawOp::Image { image, dst, alpha, blend } => {
            if !image.is_valid() || image.width == 0 || image.height == 0 {
                return;
            }
            if let Some(src_canvas) = image_canvas_cached(image) {
                // save/restore brackets both globalAlpha and the composite op,
                // so neither leaks into the next op.
                ctx.save();
                ctx.set_global_alpha(*alpha as f64);
                apply_blend(ctx, *blend);
                let _ = ctx.draw_image_with_html_canvas_element_and_dw_and_dh(
                    &src_canvas,
                    dst.x as f64,
                    dst.y as f64,
                    dst.w as f64,
                    dst.h as f64,
                );
                ctx.restore();
            }
        }
        // `DrawOp` is `#[non_exhaustive]`; future ops no-op until wired.
        _ => {}
    }
}

thread_local! {
    /// Per-thread (the wasm main thread) cache of decoded image pixels as an
    /// offscreen `<canvas>`, keyed by [`ImageSource::id`]. Building the
    /// `ImageData` + `putImageData` once and reusing the canvas across frames
    /// keeps the per-frame replay from re-uploading a static image. Never
    /// evicts — canvas authors use a small, stable set of image ids.
    static IMAGE_CANVAS_CACHE: RefCell<HashMap<u64, HtmlCanvasElement>> =
        RefCell::new(HashMap::new());

    /// Persistent `DrawOp::Layer` surfaces — an offscreen `<canvas>` per layer
    /// id, retained across frames so baked strokes survive and accumulate.
    static LAYER_CANVAS_CACHE: RefCell<HashMap<u32, HtmlCanvasElement>> =
        RefCell::new(HashMap::new());
}

/// Replay `nested` into the persistent layer `id`'s offscreen canvas (wiping
/// first if `clear`), then composite it onto `ctx` at `alpha`/`blend`.
///
/// The layer canvas matches the main backing-store size and copies the main
/// context's current transform, so nested logical-coordinate ops render at
/// device resolution; the composite is then a 1:1 device-space blit. This is
/// the CPU-raster counterpart of the vello retained-op-log layer — same
/// observable pixels (CLAUDE.md §7).
fn draw_layer(
    ctx: &CanvasRenderingContext2d,
    id: u32,
    clear: bool,
    nested: &[DrawOp],
    alpha: f32,
    blend: BlendMode,
) {
    let Some(main_canvas) = ctx.canvas() else { return };
    let (bw, bh) = (main_canvas.width(), main_canvas.height());
    if bw == 0 || bh == 0 {
        return;
    }
    let Some(layer_canvas) = layer_canvas_cached(id, bw, bh) else { return };
    let Ok(Some(obj)) = layer_canvas.get_context("2d") else { return };
    let Ok(octx) = obj.dyn_into::<CanvasRenderingContext2d>() else { return };

    if clear {
        let _ = octx.reset_transform();
        octx.clear_rect(0.0, 0.0, bw as f64, bh as f64);
    }
    // Mirror the main context's transform so nested ops (logical coords) land
    // at the same device pixels they would in the main canvas.
    if let Ok(m) = ctx.get_transform() {
        let _ = octx.set_transform(m.a(), m.b(), m.c(), m.d(), m.e(), m.f());
    }
    for op in nested {
        apply_op(&octx, op);
    }

    // Composite the layer device-for-device under alpha + blend.
    ctx.save();
    let _ = ctx.reset_transform();
    ctx.set_global_alpha(alpha as f64);
    apply_blend(ctx, blend);
    let _ = ctx.draw_image_with_html_canvas_element(&layer_canvas, 0.0, 0.0);
    ctx.restore();
}

/// Get-or-build the persistent offscreen `<canvas>` for layer `id`, sized to
/// the main backing store. A backing-store size change resizes (and thereby
/// clears) the layer — the canvas was about to be repainted at the new size
/// anyway.
fn layer_canvas_cached(id: u32, bw: u32, bh: u32) -> Option<HtmlCanvasElement> {
    LAYER_CANVAS_CACHE.with(|c| {
        if let Some(existing) = c.borrow().get(&id) {
            if existing.width() != bw || existing.height() != bh {
                existing.set_width(bw);
                existing.set_height(bh);
            }
            return Some(existing.clone());
        }
        let document = web_sys::window()?.document()?;
        let canvas: HtmlCanvasElement =
            document.create_element("canvas").ok()?.dyn_into().ok()?;
        canvas.set_width(bw);
        canvas.set_height(bh);
        c.borrow_mut().insert(id, canvas.clone());
        Some(canvas)
    })
}

/// Get-or-build the offscreen `<canvas>` holding `src`'s pixels.
fn image_canvas_cached(src: &ImageSource) -> Option<HtmlCanvasElement> {
    IMAGE_CANVAS_CACHE.with(|c| {
        if let Some(existing) = c.borrow().get(&src.id) {
            return Some(existing.clone());
        }
        let canvas = build_image_canvas(src)?;
        c.borrow_mut().insert(src.id, canvas.clone());
        Some(canvas)
    })
}

/// Paint `src`'s raw RGBA into a fresh offscreen `<canvas>` via `ImageData`.
fn build_image_canvas(src: &ImageSource) -> Option<HtmlCanvasElement> {
    let document = web_sys::window()?.document()?;
    let canvas: HtmlCanvasElement =
        document.create_element("canvas").ok()?.dyn_into().ok()?;
    canvas.set_width(src.width);
    canvas.set_height(src.height);
    let ctx: CanvasRenderingContext2d =
        canvas.get_context("2d").ok()??.dyn_into().ok()?;
    let data = ImageData::new_with_u8_clamped_array_and_sh(
        Clamped(src.rgba.as_slice()),
        src.width,
        src.height,
    )
    .ok()?;
    ctx.put_image_data(&data, 0.0, 0.0).ok()?;
    Some(canvas)
}

/// Map a [`BlendMode`] to its Canvas2D `globalCompositeOperation` string.
/// `Normal` maps to the implicit default, so callers skip touching the
/// context for it.
fn blend_css(blend: BlendMode) -> Option<&'static str> {
    match blend {
        BlendMode::Normal => None,
        BlendMode::DestinationOut => Some("destination-out"),
        BlendMode::Multiply => Some("multiply"),
        BlendMode::Screen => Some("screen"),
        // `BlendMode` is `#[non_exhaustive]`; unknown modes fall back to
        // source-over (the default), matching the documented contract.
        _ => None,
    }
}

/// Set the composite op for a blended paint. No-op for `Normal`.
fn apply_blend(ctx: &CanvasRenderingContext2d, blend: BlendMode) {
    if let Some(css) = blend_css(blend) {
        let _ = ctx.set_global_composite_operation(css);
    }
}

/// Restore source-over after a blended paint, so the next op isn't
/// silently affected. No-op when `apply_blend` did nothing.
fn clear_blend(ctx: &CanvasRenderingContext2d, blend: BlendMode) {
    if blend_css(blend).is_some() {
        let _ = ctx.set_global_composite_operation("source-over");
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
