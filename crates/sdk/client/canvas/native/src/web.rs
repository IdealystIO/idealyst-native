//! Web (`target_arch = "wasm32"`) Canvas2D rasterizer for the canvas SDK
//! — the core-free half: `Scene` → `CanvasRenderingContext2d` op replay,
//! texture-layer compositing, and `captureStream` self-capture.
//!
//! The mount handler that drives it lives in [`crate::web_scene`]; the
//! same rasterizer (`make_2d_rasterizer`) is also `canvas-vello`'s
//! WebGPU-unavailable fallback, so both paths produce identical output
//! (CLAUDE.md §7).
//!
//! Every DOM and Canvas2D call goes through web-glue ([`crate::web_ctx`]).
//! A texture layer's and the self-capture's `native_source` is a
//! `web_glue::dom::MediaStream`, the type every media producer publishes and
//! every consumer downcasts. One thing still crosses to web-sys, at the
//! crate's seam only (`HYBRID-BRIDGE: wgpu`), and only with the
//! `web-sys-canvas` feature: the public entry points `make_2d_rasterizer` /
//! `publish_capture_stream` take a `web_sys::HtmlCanvasElement`, because
//! `canvas-vello` (on wgpu, hence wasm-bindgen — hybrid mode) calls them with
//! one, so the element crosses in with `web_glue::bridge`. Without the
//! feature the crate links no wasm-bindgen and its apps build in own mode.
//!
//! [`Scene`]: canvas_core::Scene

use canvas_core::{
    BlendMode, CanvasProps, Color, DrawOp, FillRule, ImageSource, LineCap, LineJoin,
    LinearGradient, Paint, PaintKind, Path, PathSeg, RadialGradient, Scene, TextureLayer, Transform,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use web_glue::dom::{Document, HtmlCanvasElement, MediaStream, ResizeObserver};
use web_glue::Closure;

use crate::web_ctx::{capture_stream, context_2d, new_canvas, CaptureTrack, Ctx2d, Gradient, Video};

/// Disconnects the `ResizeObserver` and frees its `Closure` on scope
/// teardown, so a callback the browser has already queued can't fire
/// into freed wasm state after unmount (the classic web-listener UAF).
/// `pub(crate)`: the mount handler in `web_scene` owns the
/// resize/teardown contract.
pub(crate) struct ObserverGuard {
    pub(crate) observer: ResizeObserver,
    pub(crate) _cb: Closure,
}

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        self.observer.disconnect();
    }
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
///
/// Takes a `web_sys::HtmlCanvasElement` because that is what `canvas-vello`
/// holds; the element crosses into web-glue once, here (HYBRID-BRIDGE:
/// wgpu).
#[cfg(feature = "web-sys-canvas")]
pub fn make_2d_rasterizer(
    canvas: web_sys::HtmlCanvasElement,
    props: &Rc<CanvasProps>,
) -> Box<dyn FnMut(&Scene)> {
    // HYBRID-BRIDGE: wgpu — canvas-vello's web-sys canvas into the glue slab.
    rasterizer_2d(web_glue::bridge::from_bindgen(canvas.as_ref()), props)
}

/// [`make_2d_rasterizer`] on a web-glue canvas — what this crate's own
/// handler calls (its `<canvas>` never exists as a web-sys value).
pub(crate) fn rasterizer_2d(
    canvas: HtmlCanvasElement,
    props: &Rc<CanvasProps>,
) -> Box<dyn FnMut(&Scene)> {
    let ctx: Ctx2d = context_2d(&canvas).expect("2d context unavailable");

    let document = web_glue::dom::window()
        .expect("no window")
        .document()
        .expect("no document");

    // Texture layers (camera): a hidden <video> per layer, drawImage'd over the
    // scene so `captureStream` records it too (web parity for camera-in-canvas).
    // Persists across renders.
    let layers = props.layers.clone();
    let layer_videos: Rc<RefCell<Vec<LayerVideo>>> = Rc::new(RefCell::new(Vec::new()));

    let capture = capture_stream_of(&canvas, props);

    Box::new(move |scene: &Scene| {
        // Textures composite where the scene places them (`DrawOp::Texture`);
        // `paint_scene` has already appended any the author didn't place.
        render_scene(&canvas, &ctx, scene, &mut |ctx, index| {
            if let Some(layer) = layers.get(index as usize) {
                draw_texture(&document, ctx, index as usize, layer, &layer_videos);
            }
        });
        // Manual capture: grab the just-rendered frame (paced to CAPTURE_FPS).
        if let Some(c) = &capture {
            c.tick();
        }
    })
}

/// Publish `canvas` as a `captureStream()` [`MediaStream`] into `props.capture`
/// (web self-capture). No-op (returns `None`) when the canvas has no `capture`
/// sink. The recorder records the canvas directly — no readback.
///
/// Shared by the 2D rasterizer and `canvas-vello`'s web **GPU** path:
/// `captureStream` works on any canvas regardless of context type, so the vello
/// path reuses this instead of a GPU→CPU readback (the readback's blocking
/// `map`+`poll` is illegal on the wasm main thread). (The app must keep the
/// canvas re-rendering, e.g. a `version` raf, while recording, or the captured
/// stream is a frozen frame.)
///
/// Capture runs in **manual mode** (`frameRequestRate = 0`): a frame is produced
/// only when we call [`CanvasCaptureFrameDriver::tick`] after a render. A fixed
/// auto rate is unreliable on a **WebGPU** canvas — a swapchain *present* doesn't
/// dependably mark the canvas dirty for the browser's capture timer, so the track
/// under-delivers and the recording is choppy even while the canvas itself renders
/// at full speed. Driving `requestFrame()` per present pins one captured frame to
/// each render (paced by [`CaptureFrameDriver`]). The returned driver is `None`
/// when there's no capture sink; otherwise the caller MUST `tick()` it each frame.
///
/// Takes a `web_sys::HtmlCanvasElement` because that is what `canvas-vello`
/// holds; the element crosses into web-glue once, here (HYBRID-BRIDGE:
/// wgpu).
#[cfg(feature = "web-sys-canvas")]
#[must_use]
pub fn publish_capture_stream(
    canvas: &web_sys::HtmlCanvasElement,
    props: &CanvasProps,
) -> Option<CaptureFrameDriver> {
    // HYBRID-BRIDGE: wgpu — canvas-vello's web-sys canvas into the glue slab.
    capture_stream_of(&web_glue::bridge::from_bindgen(canvas.as_ref()), props)
}

/// [`publish_capture_stream`] on a web-glue canvas.
fn capture_stream_of(canvas: &HtmlCanvasElement, props: &CanvasProps) -> Option<CaptureFrameDriver> {
    let capture = props.capture.as_ref()?;
    let (stream, track) = capture_stream(canvas)?;
    // Consumers (media-writer, media-stream's screenshot, video) downcast
    // the native source to a `web_glue::dom::MediaStream`.
    capture.publish_native_source(Rc::new(stream));
    Some(CaptureFrameDriver { track, last_ms: std::cell::Cell::new(f64::NEG_INFINITY) })
}

/// Drives manual `captureStream` frames: call [`tick`](Self::tick) right after
/// each render/present and it issues a `requestFrame()`, throttled to
/// [`CAPTURE_FPS`] so a 120 Hz render loop doesn't over-encode.
pub struct CaptureFrameDriver {
    track: CaptureTrack,
    last_ms: std::cell::Cell<f64>,
}

impl CaptureFrameDriver {
    /// Capture the just-presented frame, unless less than one [`CAPTURE_FPS`]
    /// interval has elapsed since the last capture.
    pub fn tick(&self) {
        let now = web_glue::dom::performance_now();
        if now - self.last_ms.get() < CAPTURE_MIN_INTERVAL_MS {
            return;
        }
        self.last_ms.set(now);
        self.track.request_frame();
    }
}

/// Target frame rate for the web self-capture `captureStream()`. The render loop
/// runs faster (display refresh, often 120 Hz); we pace captured frames to this.
const CAPTURE_FPS: f64 = 60.0;
const CAPTURE_MIN_INTERVAL_MS: f64 = 1000.0 / CAPTURE_FPS;

/// A hidden `<video>` element playing one layer's stream, reused across frames
/// (creating + attaching a stream per frame would stutter).
struct LayerVideo {
    el: Video,
    /// The web `MediaStream.id` currently attached — only re-`set_src_object`
    /// when it changes (camera opened / swapped).
    stream_id: Option<String>,
}

impl LayerVideo {
    fn new(document: &Document) -> Self {
        // Muted + autoplay so a detached element plays without user gesture;
        // playsinline avoids iOS Safari fullscreen takeover.
        Self { el: Video::new_layer(document), stream_id: None }
    }

    fn ensure(&mut self, ms: &MediaStream) {
        let id = ms.id();
        if self.stream_id.as_deref() != Some(id.as_str()) {
            self.el.attach(ms.as_js());
            self.stream_id = Some(id);
        }
    }
}

/// Composite texture layer `i` at the current point of the replay. The ctx
/// carries only the dpr base transform here (`place_textures` closes every
/// author save frame before a `Texture` op), so we work in LOGICAL coordinates —
/// the same space as the rect. Crop + fit via `TextureLayer::source_rects`
/// (shared by every renderer), rounded-rect clip, opacity and border, matching
/// the GPU `LayerCompositor`.
fn draw_texture(
    document: &Document,
    ctx: &Ctx2d,
    i: usize,
    layer: &TextureLayer,
    videos: &Rc<RefCell<Vec<LayerVideo>>>,
) {
    // A layer draws from either a stream's hidden `<video>` (indexed slot) or a
    // static image rasterized to a cached offscreen `<canvas>`; both go through
    // the same crop/clip/border below.
    enum LayerSrc {
        Video(usize),
        Image(HtmlCanvasElement),
    }
    let mut vids = videos.borrow_mut();
    let (vw, vh, src) = match &layer.source {
        canvas_core::LayerSource::Stream(f) => {
            let Some(stream) = f() else { return };
            let Some(ms) = stream
                .native_source()
                .and_then(|rc| rc.downcast::<MediaStream>().ok())
            else {
                return;
            };
            while vids.len() <= i {
                vids.push(LayerVideo::new(document));
            }
            let lv = &mut vids[i];
            lv.ensure(&ms);
            let (vw, vh) = (lv.el.video_width() as f32, lv.el.video_height() as f32);
            if vw < 1.0 || vh < 1.0 {
                return; // first frames not decoded yet
            }
            (vw, vh, LayerSrc::Video(i))
        }
        canvas_core::LayerSource::Image(f) => {
            let Some(img) = f() else { return };
            if !img.is_valid() {
                return;
            }
            let Some(canvas) = image_canvas_cached(&img) else { return };
            (img.width as f32, img.height as f32, LayerSrc::Image(canvas))
        }
    };
    let (_, _, dw, dh) = (layer.rect)();
    if dw < 1.0 || dh < 1.0 {
        return;
    }
    // Shared crop + letterbox math (same on every backend).
    let ((sx, sy, sw, sh), (ox, oy, ow, oh)) = layer.source_rects(vw, vh);
    // Clip to the DRAWN rect (letterboxed for Contain) so corners round the
    // image, not the empty bars.
    let r = ((layer.corner_radius)() as f64).clamp(0.0, (ow.min(oh) as f64) * 0.5);

    ctx.save();
    ctx.set_global_alpha(layer.opacity.clamp(0.0, 1.0) as f64);
    ctx.begin_path();
    let _ = ctx.round_rect(ox as f64, oy as f64, ow as f64, oh as f64, r);
    ctx.clip(false);
    match &src {
        LayerSrc::Video(idx) => {
            let _ = ctx.draw_image_src_dst(
                vids[*idx].el.as_js(), sx as f64, sy as f64, sw as f64, sh as f64, ox as f64,
                oy as f64, ow as f64, oh as f64,
            );
        }
        LayerSrc::Image(canvas) => {
            let _ = ctx.draw_image_src_dst(
                canvas.as_js(), sx as f64, sy as f64, sw as f64, sh as f64, ox as f64, oy as f64,
                ow as f64, oh as f64,
            );
        }
    }
    // Border frame, composited WITH the image (stays locked to the moving
    // picture). Stroked on a rounded rect inset by half the width.
    let bw = layer.border_width as f64;
    if bw > 0.0 {
        let inset = bw * 0.5;
        let br = (r - inset).max(0.0);
        ctx.begin_path();
        let _ = ctx.round_rect(
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

/// Hard ceiling for one backing-store dimension. A canvas whose CSS box is not
/// size-constrained tracks its own width/height ATTRIBUTES — then the
/// `box × dpr` sync below becomes a doubling feedback loop through the
/// `ResizeObserver` (box→attr→bigger box→…) that grows the canvas to the
/// browser maximum within milliseconds and kills the page (multi-terabyte
/// allocation; observed as a silent hang-then-crash). No real display needs
/// more than this; clamping breaks the loop into a bounded, drawable state.
const MAX_BACKING_DIM: f64 = 16384.0;

/// Resize the backing store and replay `scene` into `ctx`.
fn render_scene(
    canvas: &HtmlCanvasElement,
    ctx: &Ctx2d,
    scene: &Scene,
    texture: &mut dyn FnMut(&Ctx2d, u32),
) {
    let dpr = web_glue::dom::window().map(|w| w.device_pixel_ratio()).unwrap_or(1.0);
    let css_w = canvas.client_width() as f64;
    let css_h = canvas.client_height() as f64;
    // Not laid out yet (handler runs before insertion). The ResizeObserver
    // fires once the element has a real box and we render then.
    if css_w <= 0.0 || css_h <= 0.0 {
        return;
    }
    let bw = (css_w * dpr).round().clamp(1.0, MAX_BACKING_DIM) as u32;
    let bh = (css_h * dpr).round().clamp(1.0, MAX_BACKING_DIM) as u32;
    // Setting width/height resets the context; only do it on a real change.
    if canvas.width() != bw {
        canvas.set_width(bw);
    }
    if canvas.height() != bh {
        canvas.set_height(bh);
    }

    ctx.reset_transform();
    ctx.clear_rect(0.0, 0.0, bw as f64, bh as f64);
    // Author coordinates are logical pixels; map them to device pixels.
    ctx.scale(dpr, dpr);
    // Protect the dpr base transform from an unbalanced author `restore`.
    ctx.save();
    for op in scene.ops() {
        match op {
            DrawOp::Texture { index } => texture(ctx, *index),
            op => apply_op(ctx, op),
        }
    }
    ctx.restore();
}

fn apply_op(ctx: &Ctx2d, op: &DrawOp) {
    match op {
        DrawOp::Save => ctx.save(),
        DrawOp::Restore => ctx.restore(),
        DrawOp::Transform(t) => {
            ctx.transform(
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
            ctx.fill(matches!(fill_rule, FillRule::EvenOdd));
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
            // Dash pattern via setLineDash (a JS number array); reset after.
            if !stroke.dash.is_empty() {
                let dash: Vec<f64> = stroke.dash.iter().map(|&d| d as f64).collect();
                ctx.set_line_dash(&dash);
                ctx.set_line_dash_offset(stroke.dash_offset as f64);
            }
            apply_blend(ctx, paint.blend);
            ctx.stroke();
            clear_blend(ctx, paint.blend);
            if !stroke.dash.is_empty() {
                ctx.set_line_dash(&[]);
                ctx.set_line_dash_offset(0.0);
            }
        }
        DrawOp::Clip { path, fill_rule } => {
            build_path(ctx, path);
            ctx.clip(matches!(fill_rule, FillRule::EvenOdd));
        }
        DrawOp::Layer { id, clear, ops: nested, alpha, blend } => {
            draw_layer(ctx, *id, *clear, nested, *alpha, *blend);
        }
        DrawOp::LayerCached { id, dirty, transform, ops: nested, alpha, blend } => {
            draw_cached_layer(ctx, *id, *dirty, transform, nested, *alpha, *blend);
        }
        DrawOp::Shapes { shapes, blend } => {
            // Canvas2D has no instanced fast path: expand the batch to per-shape
            // fills, in array order, replaying each through the Fill arm so a
            // batched shape and a hand-authored fill match (CLAUDE.md §7).
            for sh in shapes {
                apply_op(ctx, &sh.to_fill_op(*blend));
            }
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
                let _ = ctx.draw_image_dw_dh(
                    src_canvas.as_js(),
                    dst.x as f64,
                    dst.y as f64,
                    dst.w as f64,
                    dst.h as f64,
                );
                ctx.restore();
            }
        }
        DrawOp::Glyphs { font, glyphs, paint } => {
            // Canvas2D draws system fonts by family name, but a PDF's embedded
            // font has no system name — outline each glyph and fill it, matching
            // the GPU (vello) path's geometry (CLAUDE.md §7).
            for op in canvas_core::expand_glyph_run(font, glyphs, paint) {
                apply_op(ctx, &op);
            }
        }
        DrawOp::MaskGroup { content, .. } => {
            // No soft-mask primitive wired on Canvas2D yet: draw the content
            // unmasked so it doesn't vanish (the GPU/vello path masks correctly).
            for op in content {
                apply_op(ctx, op);
            }
        }
        // Top-level textures are composited by `render_scene`; nested ones
        // never reach a renderer (`canvas_core::place_textures` strips them).
        DrawOp::Texture { .. } => {}
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

    /// Persistent `DrawOp::LayerCached` surfaces — an offscreen `<canvas>` per
    /// layer id, baked once (`dirty`) and composited under a camera transform
    /// every frame. Distinct from [`LAYER_CANVAS_CACHE`] so the cached and
    /// accumulate layers can share an id. This is the Canvas2D **fallback** for
    /// the cached-layer fast path (the WebGPU/vello path is the ideal web path).
    static CACHED_LAYER_CANVAS_CACHE: RefCell<HashMap<u32, HtmlCanvasElement>> =
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
    ctx: &Ctx2d,
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
    let Some(octx) = context_2d(&layer_canvas) else { return };

    if clear {
        octx.reset_transform();
        octx.clear_rect(0.0, 0.0, bw as f64, bh as f64);
    }
    // Mirror the main context's transform so nested ops (logical coords) land
    // at the same device pixels they would in the main canvas.
    let [a, b, c, d, e, f] = ctx.get_transform();
    octx.set_transform(a, b, c, d, e, f);
    for op in nested {
        apply_op(&octx, op);
    }

    // Composite the layer device-for-device under alpha + blend.
    ctx.save();
    ctx.reset_transform();
    ctx.set_global_alpha(alpha as f64);
    apply_blend(ctx, blend);
    let _ = ctx.draw_image(layer_canvas.as_js(), 0.0, 0.0);
    ctx.restore();
}

/// Replay `nested` into the cached layer `id`'s offscreen canvas (only when
/// `dirty`), then composite it onto `ctx` under `transform` at `alpha`/`blend`.
///
/// The Canvas2D **fallback** counterpart of the vello `TransformCompositor`
/// (the WebGPU/vello path is the ideal web path; this runs only when WebGPU is
/// unavailable). The offscreen holds device-resolution content (baked at the
/// main context's current transform — the dpr base, since a cached layer leads
/// its scene). Compositing applies the logical `transform` in device space:
/// `setTransform(a, b, c, d, dpr·e, dpr·f)` is the conjugation of the logical
/// affine by the dpr scale (linear part unchanged; translation scaled by dpr),
/// matching the GPU path's `device = dpr · transform · logical`. `dirty: false`
/// skips the bake and just re-composites the retained raster — the cheap
/// per-frame pan/zoom path.
fn draw_cached_layer(
    ctx: &Ctx2d,
    id: u32,
    dirty: bool,
    transform: &Transform,
    nested: &[DrawOp],
    alpha: f32,
    blend: BlendMode,
) {
    let Some(main_canvas) = ctx.canvas() else { return };
    let (bw, bh) = (main_canvas.width(), main_canvas.height());
    if bw == 0 || bh == 0 {
        return;
    }
    let Some(layer_canvas) = cached_layer_canvas_cached(id, bw, bh) else { return };

    if dirty {
        let Some(octx) = context_2d(&layer_canvas) else { return };
        octx.reset_transform();
        octx.clear_rect(0.0, 0.0, bw as f64, bh as f64);
        // Bake at the main context's current transform (the dpr base for a
        // leading cached layer), so nested logical ops land at device resolution.
        let [a, b, c, d, e, f] = ctx.get_transform();
        octx.set_transform(a, b, c, d, e, f);
        for op in nested {
            apply_op(&octx, op);
        }
    }

    // Composite under the logical camera transform, conjugated by dpr (the
    // offscreen is already device-space). `setTransform` is absolute, replacing
    // the dpr base — correct because a cached layer leads its scene (no author
    // transform is active above it), matching the GPU fast path's plan gate.
    let dpr = web_glue::dom::window().map(|w| w.device_pixel_ratio()).unwrap_or(1.0);
    ctx.save();
    ctx.set_transform(
        transform.a as f64,
        transform.b as f64,
        transform.c as f64,
        transform.d as f64,
        transform.e as f64 * dpr,
        transform.f as f64 * dpr,
    );
    ctx.set_global_alpha(alpha as f64);
    apply_blend(ctx, blend);
    let _ = ctx.draw_image(layer_canvas.as_js(), 0.0, 0.0);
    ctx.restore();
}

/// Get-or-build the persistent offscreen `<canvas>` for cached layer `id`, sized
/// to the main backing store. A size change resizes (and clears) it — the app
/// re-bakes (`dirty`) on the resize repaint, same as the GPU path.
fn cached_layer_canvas_cached(id: u32, bw: u32, bh: u32) -> Option<HtmlCanvasElement> {
    CACHED_LAYER_CANVAS_CACHE.with(|c| {
        if let Some(existing) = c.borrow().get(&id) {
            if existing.width() != bw || existing.height() != bh {
                existing.set_width(bw);
                existing.set_height(bh);
            }
            return Some(existing.clone());
        }
        let document = web_glue::dom::window()?.document()?;
        let canvas = new_canvas(&document)?;
        canvas.set_width(bw);
        canvas.set_height(bh);
        c.borrow_mut().insert(id, canvas.clone());
        Some(canvas)
    })
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
        let document = web_glue::dom::window()?.document()?;
        let canvas = new_canvas(&document)?;
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
    let document = web_glue::dom::window()?.document()?;
    let canvas = new_canvas(&document)?;
    canvas.set_width(src.width);
    canvas.set_height(src.height);
    let ctx = context_2d(&canvas)?;
    ctx.put_rgba(src.rgba.as_slice(), src.width, src.height).ok()?;
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
        // The CSS `mix-blend-mode` keywords (Canvas2D accepts the same set).
        BlendMode::Overlay => Some("overlay"),
        BlendMode::Darken => Some("darken"),
        BlendMode::Lighten => Some("lighten"),
        BlendMode::ColorDodge => Some("color-dodge"),
        BlendMode::ColorBurn => Some("color-burn"),
        BlendMode::HardLight => Some("hard-light"),
        BlendMode::SoftLight => Some("soft-light"),
        BlendMode::Difference => Some("difference"),
        BlendMode::Exclusion => Some("exclusion"),
        BlendMode::Hue => Some("hue"),
        BlendMode::Saturation => Some("saturation"),
        BlendMode::Color => Some("color"),
        BlendMode::Luminosity => Some("luminosity"),
        // `BlendMode` is `#[non_exhaustive]`; unknown modes fall back to
        // source-over (the default), matching the documented contract.
        _ => None,
    }
}

/// Set the composite op for a blended paint. No-op for `Normal`.
fn apply_blend(ctx: &Ctx2d, blend: BlendMode) {
    if let Some(css) = blend_css(blend) {
        ctx.set_global_composite_operation(css);
    }
}

/// Restore source-over after a blended paint, so the next op isn't
/// silently affected. No-op when `apply_blend` did nothing.
fn clear_blend(ctx: &Ctx2d, blend: BlendMode) {
    if blend_css(blend).is_some() {
        ctx.set_global_composite_operation("source-over");
    }
}

fn build_path(ctx: &Ctx2d, path: &Path) {
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

fn apply_fill_paint(ctx: &Ctx2d, paint: &Paint) {
    match &paint.kind {
        PaintKind::Solid(c) => ctx.set_fill_style_str(&rgba_css(*c)),
        PaintKind::Linear(g) => ctx.set_fill_style_gradient(&linear_gradient(ctx, g)),
        PaintKind::Radial(g) => {
            if let Some(grad) = radial_gradient(ctx, g) {
                ctx.set_fill_style_gradient(&grad);
            }
        }
        // `PaintKind` is `#[non_exhaustive]`; unknown paints draw nothing.
        _ => ctx.set_fill_style_str("rgba(0,0,0,0)"),
    }
}

fn apply_stroke_paint(ctx: &Ctx2d, paint: &Paint) {
    match &paint.kind {
        PaintKind::Solid(c) => ctx.set_stroke_style_str(&rgba_css(*c)),
        PaintKind::Linear(g) => ctx.set_stroke_style_gradient(&linear_gradient(ctx, g)),
        PaintKind::Radial(g) => {
            if let Some(grad) = radial_gradient(ctx, g) {
                ctx.set_stroke_style_gradient(&grad);
            }
        }
        _ => ctx.set_stroke_style_str("rgba(0,0,0,0)"),
    }
}

fn linear_gradient(ctx: &Ctx2d, g: &LinearGradient) -> Gradient {
    let grad =
        ctx.create_linear_gradient(g.x0 as f64, g.y0 as f64, g.x1 as f64, g.y1 as f64);
    for s in &g.stops {
        let _ = grad.add_color_stop(s.offset, &rgba_css(s.color));
    }
    grad
}

fn radial_gradient(ctx: &Ctx2d, g: &RadialGradient) -> Option<Gradient> {
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
