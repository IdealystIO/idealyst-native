//! `canvas-core` — the renderer-agnostic 2D drawing abstraction for the
//! idealyst framework.
//!
//! This crate owns the **abstraction**: the retained [`Scene`] model
//! (paths, paint, strokes, transforms) and the [`Canvas`] primitive that
//! carries an author's draw closure into a [`CanvasPrim`] scene payload.
//! It contains **no rendering code**. Two interchangeable renderer
//! crates register a handler for [`CanvasPrim`]:
//!
//! - `canvas-native` — replays the scene with each platform's native 2D
//!   engine (web Canvas2D, iOS CoreGraphics, Android `android.graphics`).
//! - `canvas-vello` — renders the scene on a GPU surface via `vello`,
//!   for backends with no native 2D API (winit/wgpu desktop, etc.).
//!
//! An app picks a renderer at bootstrap by calling exactly one
//! `register(&mut registry)` (the registry is `TypeId`-keyed, last-wins).
//! Because both renderers consume the identical [`Scene`], swapping the
//! `register` call swaps renderers with zero changes to screen code —
//! which also makes benchmarking native-vs-vello apples-to-apples.
//!
//! # Usage
//!
//! ```ignore
//! // App bootstrap — one renderer, at the boot entry's register seam:
//! backend_web::newcore::start_in("#app", canvas_native::register, app);
//!
//! // On a screen — the "type in tag" SDK convention, small namespace:
//! use canvas::prelude::*;
//! ui! {
//!     View {
//!         { canvas::Canvas(CanvasProps {
//!             draw: canvas::draw(move |s: &mut Scene| {
//!                 s.path()
//!                  .move_to(10.0, 10.0)
//!                  .line_to(120.0, 10.0)
//!                  .cubic_to(140.0, 40.0, 90.0, 80.0, 10.0, 60.0)
//!                  .close();
//!                 s.fill(Paint::solid(Color::new(40, 120, 255, 255)));
//!                 s.stroke(Color::new(20, 20, 20, 255), Stroke::width(2.0));
//!             }),
//!             ..Default::default()
//!         }) }
//!     }
//! }
//! ```
//!
//! The `draw` closure runs inside the active backend handler's reactive
//! `Effect`, so any `Signal` read inside it re-renders the canvas on
//! change — the same reactive-source convention as `video`/`svg`.
#![deny(missing_docs)]

mod scene;
pub use scene::*;

pub mod glyph_outline;
pub use glyph_outline::expand_glyph_run;

mod prim;
pub use prim::{register_ssr, Canvas, CanvasBound, CanvasPrim, SizeReporter};

use runtime_core::{IdealystSchema, Length, StyleRules, StyleSheet};
use std::rc::Rc;
use std::sync::Arc;

/// Author-supplied props for a [`Canvas`] instance. Carried inside the
/// [`CanvasPrim`] scene payload; the active renderer's registered
/// handler reads the typed `Rc<CanvasProps>` back out and replays the
/// [`Scene`] the `draw` closure produces.
#[derive(IdealystSchema)]
pub struct CanvasProps {
    /// The scene painter. Called by the renderer (inside a reactive
    /// `Effect`) with a fresh, empty [`Scene`] to populate. Build it
    /// with [`draw`] from a closure — `&str`-style coercion isn't
    /// applicable here, the value is always a `Fn(&mut Scene)`.
    ///
    /// Reactive: signals read inside the closure re-run it and
    /// re-render the canvas. `Fn` (not `FnMut`) because the renderer
    /// may invoke it on every frame / dependency change.
    #[schema(constraint = "a Fn(&mut Scene) painter — build with canvas::draw(...)")]
    pub draw: DrawFn,

    /// Optional self-capture sink. When set, the active renderer publishes each
    /// rendered frame into this `FrameWriter` (the producer half of a
    /// [`media_stream::MediaStream`] the app holds), so the canvas's OWN output
    /// can be recorded: `let (stream, writer) = MediaStream::new();
    /// Canvas { capture: Some(writer), .. }`, then record `stream`. The renderer
    /// only does the read-back while a consumer is actually tapping frames
    /// (`writer.wants_cpu_frames()`), so an idle canvas pays nothing.
    /// `None` = no capture (the default).
    ///
    /// Captured by: the GPU renderer (`canvas-vello`) — zero-copy IOSurface on
    /// macOS, GPU→CPU read-back elsewhere; AND the CPU renderers (`canvas-native`)
    /// — `android.graphics` bitmap read-back on Android, and an offscreen
    /// CoreGraphics read-back on the iOS **simulator** (`cfg(target_abi = "sim")`,
    /// where vello can't run). On real iOS devices vello handles capture, so the
    /// CPU path isn't compiled. Web records via `captureStream`. The CPU-renderer
    /// paths are simulator/emulator fallbacks and are markedly slower — record on
    /// a physical device for representative performance.
    #[schema(constraint = "optional media_stream::FrameWriter to record the canvas output")]
    pub capture: Option<media_stream::FrameWriter>,

    /// Texture sources — each a live `MediaStream` (a camera, screen share, …)
    /// or a static image, drawn as a positioned, fitted, rounded,
    /// opacity-blended rectangle.
    ///
    /// The scene decides where each one sits in the draw order:
    /// `Scene::texture(i)` composites `layers[i]` at that point, so anything
    /// drawn after it goes on top (an outline over a camera frame). A layer
    /// the scene never places is composited after the whole scene, in order.
    /// [`paint_scene`] applies these rules once for every renderer (see
    /// [`place_textures`]).
    ///
    /// Textures are part of the rendered output, so both the on-screen canvas
    /// AND the self-capture recording show them (WYSIWYG). Every renderer
    /// composites them at their op position — the GPU vello renderer imports
    /// each stream's native surface (an IOSurface on macOS) for a zero-copy
    /// texture; the CPU renderers pull the stream's latest RGBA frame
    /// ([`MediaStream::latest`](media_stream::MediaStream::latest)) and draw it
    /// with their native 2D engine. All share
    /// [`TextureLayer::source_rects`], so crop and fit frame a layer identically
    /// across backends. Empty by default.
    #[schema(constraint = "texture layers (e.g. a camera) composited over the scene")]
    pub layers: Vec<TextureLayer>,
}

/// How a [`TextureLayer`]'s source maps into its destination rectangle.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Fit {
    /// Stretch to fill the rect exactly (may distort).
    Fill,
    /// Scale to fill the rect preserving aspect; crop the overflow (centered).
    #[default]
    Cover,
    /// Scale to fit inside the rect preserving aspect; letterbox the remainder.
    Contain,
}

impl Fit {
    /// Map a source image of size `vw × vh` into the destination rect
    /// `(dx, dy, dw, dh)`, returning `(src, dst)` where `src = (sx, sy, sw, sh)`
    /// is the sub-rectangle of the SOURCE to sample and `dst = (x, y, w, h)` is
    /// the sub-rectangle of the destination to draw into. This is the single
    /// source of truth every CPU renderer (web `drawImage`, Android
    /// `Canvas.drawBitmap`, iOS `CGContextDrawImage`) shares so a camera layer
    /// crops/letterboxes identically on every backend (the GPU vello compositor
    /// does the equivalent in UV space).
    ///
    /// - [`Fill`](Self::Fill): whole source → whole dest (may distort).
    /// - [`Cover`](Self::Cover): crop a centered slice of the source so the
    ///   whole dest is covered, aspect preserved.
    /// - [`Contain`](Self::Contain): whole source into a centered, aspect-fit
    ///   sub-rect of the dest (letterboxed remainder).
    ///
    /// Degenerate inputs (any dimension `<= 0`) return the full source → full
    /// dest, so a renderer never divides by zero.
    // The `(src, dst)` pair of `(x,y,w,h)` rect tuples is the natural shape here
    // — the same tuple `TextureLayer::rect` already uses; a named struct would
    // be heavier than the call sites warrant.
    #[allow(clippy::type_complexity)]
    pub fn map_rects(
        self,
        vw: f32,
        vh: f32,
        dx: f32,
        dy: f32,
        dw: f32,
        dh: f32,
    ) -> ((f32, f32, f32, f32), (f32, f32, f32, f32)) {
        let full = ((0.0, 0.0, vw, vh), (dx, dy, dw, dh));
        if vw <= 0.0 || vh <= 0.0 || dw <= 0.0 || dh <= 0.0 {
            return full;
        }
        match self {
            Fit::Fill => full,
            Fit::Cover => {
                // Crop a centered slice of the source matching the dest aspect.
                let s = (dw / vw).max(dh / vh);
                let (sw, sh) = (dw / s, dh / s);
                let (sx, sy) = ((vw - sw) * 0.5, (vh - sh) * 0.5);
                ((sx, sy, sw, sh), (dx, dy, dw, dh))
            }
            Fit::Contain => {
                // Whole source into a centered, aspect-fit sub-rect of the dest.
                let s = (dw / vw).min(dh / vh);
                let (ow, oh) = (vw * s, vh * s);
                let (ox, oy) = (dx + (dw - ow) * 0.5, dy + (dh - oh) * 0.5);
                ((0.0, 0.0, vw, vh), (ox, oy, ow, oh))
            }
        }
    }
}

/// What a [`TextureLayer`] draws: either a live video stream or a static image
/// (a logo/watermark). Both flow through the SAME per-backend compositor path —
/// identical fit/rounded/opacity/border handling — so an image overlay
/// composites (and records) exactly like a camera layer, just without a
/// per-frame source (`[[project_canvas_self_capture]]`).
#[derive(Clone)]
pub enum LayerSource {
    /// A live `MediaStream` (camera, screen share, a composited output), resolved
    /// every composite so a source that opens/closes/swaps is picked up
    /// reactively. On the GPU backends the stream's zero-copy `native_source`
    /// (an IOSurface on Apple) is imported; the CPU backends pull its latest RGBA.
    Stream(Rc<dyn Fn() -> Option<media_stream::MediaStream>>),
    /// A static RGBA image — a watermark/logo. Uploaded once and cached by
    /// [`ImageSource::id`]; return a NEW id/`generation` only when the pixels
    /// change. Resolved every composite so a reactive `Signal<Option<_>>` can
    /// swap or hide it. Needs no keep-alive subscription (it isn't a producer).
    Image(Rc<dyn Fn() -> Option<Arc<ImageSource>>>),
}

/// A source (a live [`MediaStream`](media_stream::MediaStream) or a static
/// [`ImageSource`]) composited into the canvas at a reactive rectangle, with a
/// fit mode, rounded corners, and opacity. See [`CanvasProps::layers`].
#[derive(Clone)]
pub struct TextureLayer {
    /// What this layer draws — a live stream or a static image. Resolved every
    /// composite so a source that opens/closes (or swaps) after the canvas is
    /// built is picked up reactively (read a `Signal` inside the closure).
    /// `None` → nothing drawn this frame.
    pub source: LayerSource,
    /// The destination rectangle `(x, y, w, h)` in the canvas's LOGICAL
    /// coordinate space (the same points the author's `Scene` uses). Read every
    /// frame, so a reactive drag position follows live. The renderer scales it
    /// by the device pixel ratio to hit the physical-pixel target.
    pub rect: Rc<dyn Fn() -> (f32, f32, f32, f32)>,
    /// How the source maps into [`rect`](Self::rect).
    pub fit: Fit,
    /// Optional NORMALIZED source crop `(x, y, w, h)` in `0.0..=1.0` — sample only
    /// this sub-rectangle of the source before applying [`fit`](Self::fit). `None`
    /// (the default) samples the whole source. Used by the `video-compose` crop
    /// op to select a region of an input video. Honored by every renderer: the
    /// GPU compositor folds it into the sampling UV, the CPU renderers sample
    /// [`source_rects`](Self::source_rects) (crop, then fit).
    pub src_crop: Option<(f32, f32, f32, f32)>,
    /// Corner radius in LOGICAL points (0 = square). Reactive — read each composite
    /// like [`rect`](Self::rect) — so a shape/size change (e.g. a camera widget
    /// toggling rounded-rect ↔ circle) updates the mask live without rebuilding the
    /// layer. Scaled to physical pixels by each backend.
    pub corner_radius: Rc<dyn Fn() -> f32>,
    /// Layer opacity `0.0..=1.0` (1 = opaque).
    pub opacity: f32,
    /// Border (frame) stroke width in LOGICAL points (0 = no border). Drawn by
    /// the renderer as a rounded-rect outline INSIDE the layer's `rect`, matching
    /// `corner_radius` — so the frame is composited together with the image and
    /// stays pixel-locked to it (e.g. a draggable camera widget whose frame must
    /// not lag the moving picture; a separate framework-view border would).
    pub border_width: f32,
    /// Border stroke color (used only when `border_width > 0`).
    pub border_color: Color,
}

impl TextureLayer {
    /// A full-opacity, square, cover-fit, border-less layer from a reactive
    /// stream source + rect.
    pub fn new(
        source: Rc<dyn Fn() -> Option<media_stream::MediaStream>>,
        rect: Rc<dyn Fn() -> (f32, f32, f32, f32)>,
    ) -> Self {
        Self::with_source(LayerSource::Stream(source), rect)
    }

    /// A full-opacity, square, cover-fit, border-less layer from a reactive
    /// static-image source + rect — the watermark/logo constructor. The image is
    /// uploaded once and cached by [`ImageSource::id`]; return a fresh id or
    /// bumped `generation` only when the pixels change.
    pub fn image(
        source: Rc<dyn Fn() -> Option<Arc<ImageSource>>>,
        rect: Rc<dyn Fn() -> (f32, f32, f32, f32)>,
    ) -> Self {
        Self::with_source(LayerSource::Image(source), rect)
    }

    fn with_source(source: LayerSource, rect: Rc<dyn Fn() -> (f32, f32, f32, f32)>) -> Self {
        Self {
            source,
            rect,
            fit: Fit::Cover,
            src_crop: None,
            corner_radius: Rc::new(|| 0.0),
            opacity: 1.0,
            border_width: 0.0,
            border_color: Color::new(0, 0, 0, 0),
        }
    }

    /// Set a NORMALIZED source crop `(x, y, w, h)` in `0.0..=1.0` — sample only
    /// this sub-rectangle of the source before fitting. See [`src_crop`](Self::src_crop).
    pub fn src_crop(mut self, crop: (f32, f32, f32, f32)) -> Self {
        self.src_crop = Some(crop);
        self
    }

    /// Set a fixed corner radius (logical points).
    pub fn corner_radius(mut self, r: f32) -> Self {
        self.corner_radius = Rc::new(move || r);
        self
    }

    /// Set a REACTIVE corner radius (logical points) — re-read each composite, so
    /// the mask follows a changing shape/size without rebuilding the layer.
    pub fn corner_radius_fn(mut self, f: impl Fn() -> f32 + 'static) -> Self {
        self.corner_radius = Rc::new(f);
        self
    }

    /// Set a border (frame) drawn with the image, in logical points. Width `0`
    /// removes it. The frame is composited with the texture, so it stays locked
    /// to the picture even while the layer's `rect` moves.
    pub fn border(mut self, width: f32, color: Color) -> Self {
        self.border_width = width;
        self.border_color = color;
        self
    }

    /// Set the fit mode.
    pub fn fit(mut self, fit: Fit) -> Self {
        self.fit = fit;
        self
    }

    /// Set the opacity (`0.0..=1.0`).
    pub fn opacity(mut self, o: f32) -> Self {
        self.opacity = o;
        self
    }

    /// The source and destination rectangles for drawing a `vw × vh` source
    /// into this layer's current [`rect`](Self::rect): the
    /// [`src_crop`](Self::src_crop) (if any) is applied first, then the
    /// [`fit`](Self::fit). Returns `(src, dst)` as in [`Fit::map_rects`], with
    /// `src` in source pixels and `dst` in canvas logical coordinates.
    ///
    /// Every CPU renderer composites a texture with these rects, and
    /// [`source_to_canvas`](Self::source_to_canvas) is derived from them, so a
    /// crop or fit frames a layer identically on every backend.
    #[allow(clippy::type_complexity)]
    pub fn source_rects(&self, vw: f32, vh: f32) -> ((f32, f32, f32, f32), (f32, f32, f32, f32)) {
        let (cx, cy, cw, ch) = self.src_crop.unwrap_or((0.0, 0.0, 1.0, 1.0));
        let (ox, oy) = (cx * vw, cy * vh);
        let (cvw, cvh) = (cw * vw, ch * vh);
        let (dx, dy, dw, dh) = (self.rect)();
        let ((sx, sy, sw, sh), dst) = self.fit.map_rects(cvw, cvh, dx, dy, dw, dh);
        ((ox + sx, oy + sy, sw, sh), dst)
    }

    /// The transform from this layer's SOURCE pixel coordinates (a `vw × vh`
    /// video frame or image) to the canvas's logical coordinates, as the layer
    /// is currently drawn (its rect, crop and fit). Use it to draw vector
    /// content registered to the picture — e.g. map a QR code's corners from
    /// frame pixels onto the composited frame:
    ///
    /// ```ignore
    /// let m = layer.source_to_canvas(scan.width as f32, scan.height as f32);
    /// let (x, y) = m.apply(corner.x, corner.y);
    /// ```
    ///
    /// Map points rather than pushing it with `Scene::transform` when stroking,
    /// so the stroke width stays in canvas units. With [`Fit::Cover`] points
    /// can map outside the drawn rect (the cropped-away part of the source).
    pub fn source_to_canvas(&self, vw: f32, vh: f32) -> Transform {
        let ((sx, sy, sw, sh), (dx, dy, dw, dh)) = self.source_rects(vw, vh);
        if sw <= 0.0 || sh <= 0.0 {
            return Transform::translate(dx, dy);
        }
        let (kx, ky) = (dw / sw, dh / sh);
        Transform::translate(-sx, -sy)
            .then(Transform::scale(kx, ky))
            .then(Transform::translate(dx, dy))
    }

    /// Resolve this layer's current pixels into `buf` for a CPU renderer
    /// (web `<canvas>`, iOS CoreGraphics, Android `drawBitmap`), returning the
    /// source `(width, height)`, or `None` if there's nothing to draw this frame
    /// (stream has no frame yet, image absent/invalid, or source is `None`). Both
    /// source kinds funnel through here so the crop/fit/opacity path downstream is
    /// identical for a live stream and a static watermark. The GPU renderer
    /// (`canvas-vello`) does NOT use this — it imports the zero-copy surface / a
    /// cached upload directly.
    pub fn resolve_rgba(&self, buf: &mut Vec<u8>) -> Option<(u32, u32)> {
        match &self.source {
            LayerSource::Stream(f) => f()?.latest(buf),
            LayerSource::Image(f) => {
                let img = f()?;
                if !img.is_valid() {
                    return None;
                }
                buf.clear();
                buf.extend_from_slice(&img.rgba);
                Some((img.width, img.height))
            }
        }
    }
}

#[doc(no_inline)]
pub use media_stream::Subscription;
/// Re-export so renderer crates (`canvas-native`, `canvas-vello`) can name the
/// self-capture sink type from `CanvasProps::capture` without a direct
/// `media-stream` dependency.
#[doc(no_inline)]
pub use media_stream::FrameWriter;

/// Keep one no-op CPU-frame subscription alive per layer whose source is
/// currently present, resizing `slots` to match `layers`.
///
/// A camera producer only does its per-frame CPU readback while
/// [`wants_cpu_frames`](media_stream::FrameWriter::wants_cpu_frames) is true —
/// i.e. while at least one consumer has an active
/// [`subscribe`](media_stream::MediaStream::subscribe). The CPU canvas renderers
/// (iOS/Android) read frames via [`latest`](media_stream::MediaStream::latest),
/// which does NOT bump that count, so without this they'd pull `None` forever.
/// This holds a throwaway subscription (the frames are consumed via `latest`,
/// not the callback) for exactly as long as each layer has a stream; dropping a
/// slot on stream-removal lets the producer stop the readback again. GPU
/// renderers that read the zero-copy native source never call this.
pub fn sync_layer_subscriptions(layers: &[TextureLayer], slots: &mut Vec<Option<Subscription>>) {
    slots.resize_with(layers.len(), || None);
    for (slot, layer) in slots.iter_mut().zip(layers.iter()) {
        // Only live-stream layers have a producer to keep warm; an image layer
        // is served from its own RGBA buffer and needs no subscription.
        let stream = match &layer.source {
            LayerSource::Stream(f) => f(),
            LayerSource::Image(_) => None,
        };
        match (stream, slot.is_some()) {
            (Some(stream), false) => *slot = Some(stream.subscribe(|_| {})),
            (None, true) => *slot = None,
            _ => {}
        }
    }
}

/// The boxed scene-painter closure [`CanvasProps::draw`] holds. Build
/// one with [`draw`].
pub type DrawFn = Box<dyn Fn(&mut Scene)>;

impl Default for CanvasProps {
    fn default() -> Self {
        // A no-op painter renders an empty canvas rather than panicking
        // on an unset field.
        Self { draw: Box::new(|_| {}), capture: None, layers: Vec::new() }
    }
}

/// Coerce a `Fn(&mut Scene)` closure into the [`DrawFn`] boxed shape
/// [`CanvasProps::draw`] expects. Mirrors `video::url` / `svg::markup`
/// — the small adapter that keeps call sites from writing `Box::new`.
///
/// ```ignore
/// CanvasProps { draw: canvas::draw(|s| { s.path()...; s.fill(paint); }), ..Default::default() }
/// ```
pub fn draw<F: Fn(&mut Scene) + 'static>(f: F) -> DrawFn {
    Box::new(f)
}

/// Render the props' painter into a fresh [`Scene`] snapshot. Renderer
/// handlers call this (inside their reactive effect) to obtain the
/// scene to replay; the wire serializer calls it to capture a static
/// snapshot for transport.
///
/// The result is normalized by [`place_textures`] against `props.layers`, so
/// every renderer receives the same op list: texture layers at their placed
/// positions (unplaced ones appended), each with the base transform/clip state.
pub fn paint_scene(props: &CanvasProps) -> Scene {
    paint_scene_sized(props, (0.0, 0.0))
}

/// [`paint_scene`] for a canvas of logical size `size`, which the painter
/// reads with [`Scene::size`]. Renderers call it through
/// [`CanvasPrim::paint`], which supplies the size the canvas last reported.
pub fn paint_scene_sized(props: &CanvasProps, size: (f32, f32)) -> Scene {
    let mut scene = Scene::with_size(size.0, size.1);
    (props.draw)(&mut scene);
    place_textures(scene, props.layers.len())
}

/// Normalize a painted scene's [`DrawOp::Texture`] ops against a canvas with
/// `layer_count` texture layers. This is the ONE place the texture ordering
/// rules live, so renderers only ever replay ops in order:
///
/// 1. A `Texture` op nested inside a `Layer` / `LayerCached` / `MaskGroup` op
///    list, or whose index names no layer, is removed.
/// 2. Every layer the scene never placed is appended after the scene, in
///    `layers` order (the behavior from before texture ops existed).
/// 3. Around each top-level `Texture` op the scene's state is reset and then
///    restored: the rewrite wraps the scene in an outer `Save`, closes every
///    open save frame (`Restore`s) just before the texture, and re-opens the
///    same frames (re-emitting their `Transform` / `Clip` ops) right after. So
///    every renderer composites the texture with the BASE transform and no
///    clip, the author's state continues unchanged after it, and each run of
///    vector ops between textures is self-contained — which a GPU renderer
///    that draws those runs as separate passes depends on.
///
/// An author `Restore` with no matching `Save` is dropped (it would otherwise
/// pop the outer frame). A scene with no textures to place is returned as is.
pub fn place_textures(scene: Scene, layer_count: usize) -> Scene {
    let size = scene.size();
    let mut placed_scene = place_texture_ops(scene.into_ops(), layer_count);
    placed_scene.set_size(size);
    placed_scene
}

fn place_texture_ops(ops: Vec<DrawOp>, layer_count: usize) -> Scene {
    let in_range = |index: u32| (index as usize) < layer_count;
    let mut placed = vec![false; layer_count];
    for op in &ops {
        if let DrawOp::Texture { index } = op {
            if in_range(*index) {
                placed[*index as usize] = true;
            }
        }
    }
    // Any layer means at least one texture is placed (explicitly or appended);
    // without layers, only stray Texture ops (out of range / nested) need
    // removing.
    let needs_rewrite = layer_count > 0
        || ops.iter().any(|o| matches!(o, DrawOp::Texture { .. }) || op_nests_texture(o));
    if !needs_rewrite {
        return Scene::from_ops(ops);
    }

    // `frames[k]` = the Transform/Clip ops issued inside save frame k (0 = the
    // outer frame this rewrite adds).
    let mut frames: Vec<Vec<DrawOp>> = vec![Vec::new()];
    let mut out = Vec::with_capacity(ops.len() + 2 + layer_count * 2);
    out.push(DrawOp::Save);

    let place = |out: &mut Vec<DrawOp>, frames: &[Vec<DrawOp>], index: u32| {
        out.extend(std::iter::repeat_n(DrawOp::Restore, frames.len()));
        out.push(DrawOp::Texture { index });
        for frame in frames {
            out.push(DrawOp::Save);
            out.extend(frame.iter().cloned());
        }
    };

    for op in ops {
        match op {
            DrawOp::Texture { index } => {
                if in_range(index) {
                    place(&mut out, &frames, index);
                }
            }
            DrawOp::Save => {
                frames.push(Vec::new());
                out.push(DrawOp::Save);
            }
            DrawOp::Restore => {
                if frames.len() > 1 {
                    frames.pop();
                    out.push(DrawOp::Restore);
                }
            }
            DrawOp::Transform(_) | DrawOp::Clip { .. } => {
                frames.last_mut().expect("outer frame").push(op.clone());
                out.push(op);
            }
            other => out.push(strip_nested_textures(other)),
        }
    }
    for (index, was_placed) in placed.iter().enumerate() {
        if !was_placed {
            place(&mut out, &frames, index as u32);
        }
    }
    out.extend(std::iter::repeat_n(DrawOp::Restore, frames.len()));
    Scene::from_ops(out)
}

/// Whether `op` carries a `Texture` op somewhere in a nested op list.
fn op_nests_texture(op: &DrawOp) -> bool {
    let any = |ops: &[DrawOp]| {
        ops.iter().any(|o| matches!(o, DrawOp::Texture { .. }) || op_nests_texture(o))
    };
    match op {
        DrawOp::Layer { ops, .. } | DrawOp::LayerCached { ops, .. } => any(ops),
        DrawOp::MaskGroup { content, mask, .. } => any(content) || any(mask),
        _ => false,
    }
}

/// Remove `Texture` ops from `op`'s nested op lists (recursively).
fn strip_nested_textures(op: DrawOp) -> DrawOp {
    fn strip(ops: Vec<DrawOp>) -> Vec<DrawOp> {
        ops.into_iter()
            .filter(|o| !matches!(o, DrawOp::Texture { .. }))
            .map(strip_nested_textures)
            .collect()
    }
    match op {
        DrawOp::Layer { id, clear, ops, alpha, blend } => {
            DrawOp::Layer { id, clear, ops: strip(ops), alpha, blend }
        }
        DrawOp::LayerCached { id, dirty, transform, ops, alpha, blend } => {
            DrawOp::LayerCached { id, dirty, transform, ops: strip(ops), alpha, blend }
        }
        DrawOp::MaskGroup { content, mask, luminance, alpha, blend } => DrawOp::MaskGroup {
            content: strip(content),
            mask: strip(mask),
            luminance,
            alpha,
            blend,
        },
        other => other,
    }
}

/// Default "fill the parent box" style for an unstyled canvas, built
/// once and shared. A canvas has no intrinsic content size, so without
/// this an unstyled `Canvas(...)` collapses to a 0×0 box on the native
/// backends (web's `<canvas>` defaults to `0×0` too once it's an
/// external element rather than the `graphics` primitive). The default
/// matches the framework's canonical fill convention used by the
/// navigators (`flex_grow: 1` + `100% × 100%`) so the same style drives
/// Taffy on native and CSS on web — identical layout input, identical
/// output (CLAUDE.md §7).
///
/// **Caveat (inherent to flexbox, not a canvas quirk):** `100%` height
/// only resolves against a parent with a *definite* height. A canvas
/// nested under auto-height flex parents needs either a sized ancestor
/// or `flex_grow` on the chain — the same rule every percentage-sized
/// box follows. `flex_grow: 1` in this default covers the common
/// "fill the remaining main-axis space" case without a definite parent.
pub(crate) fn default_fill_style() -> Rc<StyleSheet> {
    thread_local! {
        static SHEET: Rc<StyleSheet> = {
            let mut fill = StyleRules::default();
            fill.flex_grow = Some(1.0f32.into());
            fill.width = Some(Length::pct(100.0).into());
            fill.height = Some(Length::pct(100.0).into());
            Rc::new(StyleSheet::r#static(fill))
        };
    }
    SHEET.with(|s| s.clone())
}

/// One-stop import for typical screen code: brings in the [`Canvas`]
/// constructor, [`CanvasProps`], the [`draw`] coercion helper, and the
/// scene-model types ([`Scene`], [`Path`], [`Paint`], [`Stroke`],
/// [`Color`], …).
pub mod prelude {
    pub use super::{draw, Canvas, CanvasProps, Fit, LayerSource, TextureLayer};
    pub use crate::scene::{
        color, Color, FillRule, FontResource, GradientStop, LineCap, LineJoin, LinearGradient,
        Paint, PaintKind, Path, PathSeg, PositionedGlyph, RadialGradient, Scene, Stroke, Transform,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_props_paint_to_empty_scene() {
        let props = CanvasProps::default();
        assert!(paint_scene(&props).is_empty());
    }

    #[test]
    fn draw_helper_produces_scene_ops() {
        let props = CanvasProps {
            draw: draw(|s| {
                s.path().add_path(Path::rect(0.0, 0.0, 10.0, 10.0));
                s.fill(Color::new(255, 0, 0, 255));
            }),
            ..Default::default()
        };
        assert_eq!(paint_scene(&props).ops().len(), 1);
    }

    fn layer(rect: (f32, f32, f32, f32)) -> TextureLayer {
        let img = Arc::new(ImageSource::from_rgba8(1, 1, 1, vec![0, 0, 0, 255]));
        TextureLayer::image(Rc::new(move || Some(img.clone())), Rc::new(move || rect))
    }

    fn props(draw_fn: impl Fn(&mut Scene) + 'static, layers: usize) -> CanvasProps {
        CanvasProps {
            draw: draw(draw_fn),
            layers: (0..layers).map(|_| layer((0.0, 0.0, 10.0, 10.0))).collect(),
            ..Default::default()
        }
    }

    fn fill() -> DrawOp {
        DrawOp::Fill {
            path: Path::rect(0.0, 0.0, 1.0, 1.0),
            paint: Color::new(255, 0, 0, 255).into(),
            fill_rule: FillRule::NonZero,
        }
    }

    fn rect_fill(s: &mut Scene) {
        s.fill_path(Path::rect(0.0, 0.0, 1.0, 1.0), Color::new(255, 0, 0, 255));
    }

    /// Before texture ops existed every layer composited after the scene.
    /// A scene that places none must keep exactly that order.
    #[test]
    fn unplaced_layers_are_appended_after_the_scene_in_order() {
        let ops = paint_scene(&props(rect_fill, 2)).into_ops();
        assert_eq!(
            ops,
            vec![
                DrawOp::Save,
                fill(),
                DrawOp::Restore,
                DrawOp::Texture { index: 0 },
                DrawOp::Save,
                DrawOp::Restore,
                DrawOp::Texture { index: 1 },
                DrawOp::Save,
                DrawOp::Restore,
            ]
        );
    }

    /// The point of the op: content drawn after `texture(i)` is on top of it,
    /// and a placed layer is not appended a second time.
    #[test]
    fn a_placed_layer_draws_at_its_position_and_is_not_repeated() {
        let ops = paint_scene(&props(
            |s| {
                s.texture(0);
                rect_fill(s);
            },
            1,
        ))
        .into_ops();
        assert_eq!(
            ops,
            vec![
                DrawOp::Save,
                DrawOp::Restore,
                DrawOp::Texture { index: 0 },
                DrawOp::Save,
                fill(),
                DrawOp::Restore,
            ]
        );
    }

    /// Every renderer must see a texture with the base transform and no clip,
    /// and the author's state must resume after it. The rewrite closes all
    /// open frames before the texture and re-opens them (same transforms and
    /// clips) after it.
    #[test]
    fn texture_sees_base_state_and_author_state_resumes_after_it() {
        let t1 = Transform::translate(5.0, 0.0);
        let t2 = Transform::scale(2.0, 2.0);
        let clip_path = Path::rect(0.0, 0.0, 4.0, 4.0);
        let cp = clip_path.clone();
        let ops = paint_scene(&props(
            move |s| {
                s.transform(t1);
                s.save();
                s.transform(t2);
                s.add_path(cp.clone()).clip();
                s.texture(0);
                rect_fill(s);
                s.restore();
            },
            1,
        ))
        .into_ops();
        let clip = DrawOp::Clip { path: clip_path, fill_rule: FillRule::NonZero };
        assert_eq!(
            ops,
            vec![
                DrawOp::Save,
                DrawOp::Transform(t1),
                DrawOp::Save,
                DrawOp::Transform(t2),
                clip.clone(),
                // texture: both frames closed → base state
                DrawOp::Restore,
                DrawOp::Restore,
                DrawOp::Texture { index: 0 },
                // both frames re-opened with their state
                DrawOp::Save,
                DrawOp::Transform(t1),
                DrawOp::Save,
                DrawOp::Transform(t2),
                clip,
                fill(),
                DrawOp::Restore, // author's restore
                DrawOp::Restore, // outer frame
            ]
        );
    }

    #[test]
    fn out_of_range_and_nested_textures_are_removed() {
        let ops = paint_scene(&props(
            |s| {
                s.texture(7);
                s.layer(1, true, |inner| {
                    inner.texture(0);
                    rect_fill(inner);
                });
            },
            0,
        ))
        .into_ops();
        assert_eq!(
            ops,
            vec![
                DrawOp::Save,
                DrawOp::Layer {
                    id: 1,
                    clear: true,
                    ops: vec![fill()],
                    alpha: 1.0,
                    blend: BlendMode::Normal
                },
                DrawOp::Restore,
            ]
        );
    }

    /// An unbalanced author `Restore` must not pop the rewrite's outer frame.
    #[test]
    fn an_unbalanced_restore_is_dropped() {
        let ops = paint_scene(&props(
            |s| {
                s.restore();
                s.texture(0);
            },
            1,
        ))
        .into_ops();
        assert_eq!(
            ops,
            vec![DrawOp::Save, DrawOp::Restore, DrawOp::Texture { index: 0 }, DrawOp::Save, DrawOp::Restore]
        );
    }

    #[test]
    fn a_scene_without_layers_or_textures_is_untouched() {
        let ops = paint_scene(&props(rect_fill, 0)).into_ops();
        assert_eq!(ops, vec![fill()]);
    }

    /// `source_to_canvas` must land source pixels exactly where the layer's
    /// fit puts them, so an overlay registers to the picture.
    #[test]
    fn source_to_canvas_matches_the_drawn_frame() {
        // 200×100 source in a 100×100 rect at (10, 20).
        let contain = layer((10.0, 20.0, 100.0, 100.0)).fit(Fit::Contain);
        let m = contain.source_to_canvas(200.0, 100.0);
        // Contain letterboxes to (10, 45, 100, 50): corners map onto it.
        assert_eq!(m.apply(0.0, 0.0), (10.0, 45.0));
        assert_eq!(m.apply(200.0, 100.0), (110.0, 95.0));

        let cover = layer((10.0, 20.0, 100.0, 100.0)).fit(Fit::Cover);
        let m = cover.source_to_canvas(200.0, 100.0);
        // Cover samples source x 50..150 across the full rect.
        assert_eq!(m.apply(50.0, 0.0), (10.0, 20.0));
        assert_eq!(m.apply(150.0, 100.0), (110.0, 120.0));
    }

    /// `src_crop` applies before fit, on every renderer (they all composite
    /// through `source_rects`).
    #[test]
    fn source_rects_apply_the_crop_before_the_fit() {
        let l = layer((0.0, 0.0, 50.0, 50.0)).fit(Fit::Fill).src_crop((0.5, 0.0, 0.5, 1.0));
        let (src, dst) = l.source_rects(200.0, 100.0);
        assert_eq!(src, (100.0, 0.0, 100.0, 100.0));
        assert_eq!(dst, (0.0, 0.0, 50.0, 50.0));
        let (x, _) = l.source_to_canvas(200.0, 100.0).apply(100.0, 0.0);
        assert_eq!(x, 0.0);
    }

    /// `Scene::size` is the painter's view of the canvas: nested layer
    /// builders see the same size, and the texture rewrite keeps it.
    #[test]
    fn painted_size_reaches_nested_scenes_and_survives_place_textures() {
        let nested = Rc::new(std::cell::Cell::new((0.0f32, 0.0f32)));
        let n = nested.clone();
        let scene = paint_scene_sized(
            &props(
                move |s| {
                    s.layer(1, true, |inner| n.set(inner.size()));
                    s.texture(0);
                },
                1,
            ),
            (320.0, 200.0),
        );
        assert_eq!(scene.size(), (320.0, 200.0));
        assert_eq!(nested.get(), (320.0, 200.0));
    }

    /// `Fit::map_rects` is the shared crop/letterbox math every CPU renderer
    /// (web/iOS/Android) uses, so a camera layer composites identically across
    /// backends. A bug here would silently diverge one platform's framing.
    #[test]
    fn fit_map_rects_matches_per_mode_geometry() {
        // Source 200×100 (2:1), dest 100×100 (square) at origin (10, 20).
        let (vw, vh) = (200.0, 100.0);
        let (dx, dy, dw, dh) = (10.0, 20.0, 100.0, 100.0);

        // Fill: whole source → whole dest (distorts).
        let (src, dst) = Fit::Fill.map_rects(vw, vh, dx, dy, dw, dh);
        assert_eq!(src, (0.0, 0.0, 200.0, 100.0));
        assert_eq!(dst, (10.0, 20.0, 100.0, 100.0));

        // Cover: crop a centered square (100×100) of the source; full dest.
        let (src, dst) = Fit::Cover.map_rects(vw, vh, dx, dy, dw, dh);
        assert_eq!(src, (50.0, 0.0, 100.0, 100.0));
        assert_eq!(dst, (10.0, 20.0, 100.0, 100.0));

        // Contain: whole source into a centered 100×50 letterboxed sub-rect.
        let (src, dst) = Fit::Contain.map_rects(vw, vh, dx, dy, dw, dh);
        assert_eq!(src, (0.0, 0.0, 200.0, 100.0));
        assert_eq!(dst, (10.0, 45.0, 100.0, 50.0));

        // Degenerate source (no frame yet) → full→full, never divides by zero.
        let (src, dst) = Fit::Cover.map_rects(0.0, 0.0, dx, dy, dw, dh);
        assert_eq!(src, (0.0, 0.0, 0.0, 0.0));
        assert_eq!(dst, (10.0, 20.0, 100.0, 100.0));
    }

    /// An image layer resolves its pixels through the shared CPU path (used by
    /// the web/iOS/Android renderers) exactly like a stream frame would, so a
    /// watermark composites through the same crop/fit code as a camera.
    #[test]
    fn image_layer_resolves_rgba() {
        let img = Arc::new(ImageSource::from_rgba8(9, 2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]));
        let layer = TextureLayer::image(
            Rc::new(move || Some(img.clone())),
            Rc::new(|| (0.0, 0.0, 10.0, 10.0)),
        );
        let mut buf = Vec::new();
        assert_eq!(layer.resolve_rgba(&mut buf), Some((2, 1)));
        assert_eq!(buf, vec![1, 2, 3, 4, 5, 6, 7, 8]);

        // An absent image → nothing to draw this frame.
        let none = TextureLayer::image(Rc::new(|| None), Rc::new(|| (0.0, 0.0, 1.0, 1.0)));
        assert_eq!(none.resolve_rgba(&mut buf), None);
    }

    /// Image layers are not producers, so `sync_layer_subscriptions` must never
    /// allocate a keep-alive subscription slot for them (only live streams need
    /// one to keep their CPU readback warm).
    #[test]
    fn image_layer_needs_no_subscription() {
        let img = Arc::new(ImageSource::from_rgba8(1, 1, 1, vec![0, 0, 0, 255]));
        let layers = vec![TextureLayer::image(
            Rc::new(move || Some(img.clone())),
            Rc::new(|| (0.0, 0.0, 1.0, 1.0)),
        )];
        let mut slots: Vec<Option<Subscription>> = Vec::new();
        sync_layer_subscriptions(&layers, &mut slots);
        assert_eq!(slots.len(), 1);
        assert!(slots[0].is_none(), "an image layer must not hold a subscription");
    }

    /// The `Scene` wire format is what a dev-session recorder ships for a
    /// canvas (a painter closure can't cross a process boundary, so the
    /// transported artifact is a painted snapshot). Round-trip a
    /// non-trivial scene through it and confirm the replayed ops match.
    #[test]
    fn scene_snapshot_round_trips_through_the_wire_format() {
        let props = CanvasProps {
            draw: draw(|s| {
                s.path().add_path(Path::circle(20.0, 20.0, 15.0));
                s.fill(Paint::solid(Color::new(10, 20, 30, 255)));
                s.stroke(Color::new(0, 0, 0, 255), Stroke::width(3.0));
            }),
            ..Default::default()
        };

        let bytes = serde_json::to_vec(&paint_scene(&props)).expect("serialize");
        let decoded: Scene = serde_json::from_slice(&bytes).expect("deserialize");

        // A canvas rebuilt from the snapshot replays the same ops as the
        // original painter produced.
        let replay = Rc::new(decoded);
        let replayed = CanvasProps {
            draw: draw(move |s: &mut Scene| *s = (*replay).clone()),
            ..Default::default()
        };
        assert_eq!(paint_scene(&props).ops(), paint_scene(&replayed).ops());
    }
}
