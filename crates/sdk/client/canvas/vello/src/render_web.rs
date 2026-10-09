//! Web (wasm32) vello renderer with a per-canvas Canvas2D fallback.
//!
//! Renders a `canvas_core::Scene` with vello over wgpu's **WebGPU** backend.
//! WebGPU is not universal, and a `<canvas>` is permanently bound to its first
//! context type (see `gpu_surface`'s web module), so each canvas decides for
//! itself in `on_ready` (**lazy per-canvas context selection**):
//!
//! - [`gpu_surface::WebGpuProbe`] acquires an adapter + device headlessly, and
//!   the vello `Renderer` is built on that device. None of this touches the
//!   canvas, so it stays unclaimed.
//! - Only once the device AND the vello pipeline are in hand does the probe
//!   `claim` the canvas for webgpu.
//! - If anything fails first (no adapter / weak GPU / `Renderer::new` error),
//!   the still-unclaimed canvas goes to `canvas-native`'s `make_2d_rasterizer`,
//!   which renders the same output via Canvas2D — same element, no node-swap,
//!   never blank (CLAUDE.md §7). vello needs compute shaders, so WebGL2 is not
//!   an option here.
//!
//! `register` also gates on `navigator.gpu` synchronously, so browsers with no
//! WebGPU at all never override the `canvas-native` handler and pay no probe.
//!
//! Texture layers (camera-in-canvas) are composited on the GPU path by
//! [`WebLayerCompositor`], so a layered canvas stays on WebGPU. Self-capture uses
//! `captureStream()` on both paths (it works on a webgpu-context canvas) — no
//! GPU→CPU readback, whose blocking `map`+`poll` would be illegal on the wasm
//! main thread.

use crate::compose_transform::TransformCompositor;
use crate::anim::AnimTextures;
use crate::encode::encode_scene;
use crate::overscan::{overscan_dims, overscan_frac};
use crate::plan::{plan_scene, split_segments, CachedRef, ScenePlan};
use crate::texture_runs::{composite_texture_runs, RunHost};
use crate::shape_pass::ShapePass;
use crate::web_layer::WebLayerCompositor;
use crate::{LABEL, VELLO_REQUIREMENTS};
use canvas_core::{CanvasPrim, CanvasProps, DrawOp, Scene as CanvasScene, TextureLayer};
use gpu_surface::{
    marker, FrameAlpha, GpuSurface, OverlayCompositor, RenderFn, SurfaceState, WebGpuProbe,
};
use runtime_scene::{Element, Host, MountCx, Registry};
use runtime_shared::primitives::graphics::OnReadyEvent;
use runtime_vocabulary::caps::GraphicsOps;
use runtime_vocabulary::style_attach::{attach_style, on_teardown, StyleServices};

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use vello::kurbo::Affine;
use vello::peniko::Color;
use vello::{AaConfig, AaSupport, Renderer, RendererOptions, RenderParams, Scene as VelloScene};

/// Register the web vello canvas renderer on a scene registry. Overrides
/// a `canvas-native` registration for the same payload (last write wins)
/// only when the browser exposes WebGPU at all; the per-GPU viability
/// decision is deferred to each canvas's `on_ready`.
pub fn register<H>(registry: &mut Registry<H>)
where
    H: GraphicsOps + StyleServices + 'static,
{
    // Synchronous, deterministic gate. Absent `navigator.gpu` → leave
    // whatever renderer is already installed; no wasted async probe. A
    // *present* `navigator.gpu` can still fail to yield an adapter (driver
    // blocklists, VMs); that case is handled per-canvas by the Canvas2D
    // fallback in `on_ready`.
    if !gpu_surface::webgpu_present() {
        return;
    }
    registry.register::<CanvasPrim, _>(mount_canvas::<H>);
}

/// Queue this renderer's [`CanvasPrim`] handler for registration from a
/// lazily-loaded chunk, instead of installing it at boot.
///
/// The late-binding sibling of [`register`], for an app that code-splits the
/// screen its canvas lives on. Registering eagerly anchors this crate (wgpu +
/// vello + naga) in the initial bundle, because wasm-split cannot move a
/// boot-reachable handler into a chunk; called from inside the chunk, the
/// handler — and the renderer it reaches — is constructed there instead.
///
/// Requires the app to have declared `Registry::defer::<CanvasPrim>()` in its
/// boot seam. A canvas the scene meets before this runs parks behind a
/// layout-transparent placeholder and realizes on the drain.
///
/// Carries the same `navigator.gpu` gate as [`register`]: with WebGPU absent
/// this queues nothing, leaving whatever renderer the app installed (typically
/// `canvas-native`'s Canvas2D rasterizer) in place.
///
/// Generic over the host for the same reason [`register`] is — this crate takes
/// no backend dependency (its renderer is pure wgpu, so there is no
/// per-platform module to name). The caller pins `H` to its concrete backend,
/// e.g. `register_from_chunk::<backend_web::WebBackend>()`.
pub fn register_from_chunk<H>()
where
    H: Host + GraphicsOps + StyleServices + 'static,
{
    if !gpu_surface::webgpu_present() {
        return;
    }
    runtime_scene::defer_registration::<H, _>(|registry| {
        registry.register_deferred::<CanvasPrim, _>(mount_canvas::<H>);
    });
}

fn mount_canvas<H>(
    cx: &mut MountCx<'_, H>,
    prim: &Rc<CanvasPrim>,
    _children: Vec<Element>,
) -> H::Node
where
    H: GraphicsOps + StyleServices,
{
    let backend = cx.backend().clone();
    let node = {
        let mut b = backend.borrow_mut();
        build_canvas(prim, &mut *b)
    };
    finish_mount(&backend, &node, prim);
    node
}

/// Shared mount tail: author style onto the graphics node, then the
/// scope-tied `release_external` teardown (every external mount releases
/// at unmount, handler-backed or not).
fn finish_mount<H>(backend: &Rc<RefCell<H>>, node: &H::Node, prim: &CanvasPrim)
where
    H: GraphicsOps + StyleServices,
{
    if let Some(style) = prim.take_style() {
        attach_style(backend, node, style);
    }
    let backend = backend.clone();
    let node = node.clone();
    on_teardown(move || {
        backend.borrow_mut().release_external(&node);
    });
}


/// Debug-only E2E escape hatch: `window.__IDEALYST_FORCE_CANVAS2D = true` forces
/// the Canvas2D fallback so the async-bootstrap fallback branch can be exercised
/// without a blocklisted GPU. `false` in release builds.
fn force_canvas2d() -> bool {
    gpu_surface::debug_flag("__IDEALYST_FORCE_CANVAS2D")
}

/// The graphics node + lifecycle come from `gpu_surface::mount` (reactive
/// paint, rAF-paced repaint, logical size reporting); each `on_ready` resolves
/// the canvas's renderer with [`build_render_fn`].
pub fn build_canvas<H: GraphicsOps>(prim: &Rc<CanvasPrim>, backend: &mut H) -> H::Node {
    let sizing = prim.size_reporter();
    let paint_prim = prim.clone();
    let props = prim.props.clone();
    gpu_surface::mount(
        backend,
        move || paint_prim.paint(),
        move |w, h| sizing.report(w, h),
        move |ev: OnReadyEvent| {
            let props = props.clone();
            Box::pin(async move { Some(build_render_fn(ev, props).await) })
        },
    )
}

/// Decide the renderer for one canvas: vello GPU when WebGPU is viable (texture
/// layers included — `WebLayerCompositor`), else canvas-native's Canvas2D
/// rasterizer on the same (still-unclaimed) element.
async fn build_render_fn(ev: OnReadyEvent, props: Rc<CanvasProps>) -> RenderFn<CanvasScene> {
    let canvas = match ev.surface().and_then(gpu_surface::canvas_from_surface) {
        Some(c) => c,
        // Never on web: the backend always yields a `RawWindow` canvas target.
        // Degrade to a no-op rather than panic in an async task.
        None => return Box::new(|_| {}),
    };

    if force_canvas2d() {
        marker("canvas-vello: Canvas2D forced (__IDEALYST_FORCE_CANVAS2D set)");
    } else if let Some(probe) = WebGpuProbe::run(VELLO_REQUIREMENTS, LABEL).await {
        // Build the vello pipeline BEFORE claiming the canvas: this is the last
        // step that can fail on a too-weak GPU, and the canvas must still be
        // pristine for Canvas2D if it does.
        match Renderer::new(
            &probe.device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: None,
                pipeline_cache: None,
            },
        ) {
            Ok(renderer) => {
                if let Some(gpu) = probe.claim(ev, FrameAlpha::Straight) {
                    // Self-capture works on a webgpu-context canvas via
                    // captureStream — and the camera is composited INTO the
                    // canvas, so it's in the recording. Manual capture mode:
                    // `tick()` after each present, because a WebGPU present
                    // doesn't reliably trigger the browser's auto-capture timer.
                    let capture = canvas_native::publish_capture_stream(&canvas, &props);
                    let mut render = gpu_surface::drive(GpuState::new(gpu, renderer, props.layers.clone()));
                    return Box::new(move |scene: &CanvasScene| {
                        render(scene);
                        if let Some(c) = &capture {
                            c.tick();
                        }
                    });
                }
            }
            Err(e) => marker(&format!("canvas-vello: vello Renderer::new failed ({e:?})")),
        }
    }

    marker("canvas-vello: web GPU unavailable — Canvas2D fallback");
    canvas_native::make_2d_rasterizer(canvas, &props)
}

// ============================================================================
// GPU render state (web)
// ============================================================================

struct GpuState {
    /// Device, configured surface and frame target (re-synced to the canvas
    /// backing store each frame by `gpu_surface::drive`).
    gpu: GpuSurface,
    renderer: Renderer,
    /// Second vello renderer used ONLY to bake cached layers containing images.
    /// vello keeps one persistent image atlas per `Renderer`, resized to fit each
    /// baked scene's images; baking an image-less layer (grid/ink) through the
    /// same renderer shrinks the atlas to 1×1, and a later image bake resizes it
    /// back WITHOUT re-uploading the cached image → the image renders blank.
    /// Image layers on their own renderer keep their atlas intact. Lazily built.
    image_renderer: Option<Renderer>,
    /// Animated-image (video frame pump) override textures — one per animated
    /// id, written per changed frame and read by vello via `override_image`.
    /// See `anim.rs` / the animated branch of `encode::image_data_cached`.
    anim: AnimTextures,
    scene: VelloScene,
    /// Texture layers (the camera) composited over the scene each frame via
    /// [`WebLayerCompositor`]. Empty when the canvas has no layers.
    layers: Vec<TextureLayer>,
    layer_compositor: Option<WebLayerCompositor>,
    /// Instanced analytic-shape pass for a leading shape backdrop (the hybrid
    /// path), built lazily the first time a shape-led scene is rendered.
    shape_pass: Option<ShapePass>,
    /// Secondary target vello renders a HYBRID scene's `rest` into (over a
    /// transparent base); [`OverlayCompositor`] lays it over the instanced
    /// backdrop in the frame target. Lazily created, invalidated on resize.
    overlay: Option<(wgpu::Texture, wgpu::TextureView)>,
    overlay_compositor: Option<OverlayCompositor>,
    /// Baked, viewport-sized textures for `DrawOp::LayerCached`, keyed by layer
    /// id — the infinite pan/zoom fast path on WebGPU (the ideal web path;
    /// Canvas2D is only the no-WebGPU fallback). Re-rendered on a `dirty` bake,
    /// composited under the camera transform every frame. Cleared on resize.
    cached_layers: HashMap<u32, (wgpu::Texture, wgpu::TextureView)>,
    /// Last DIRTY ops per cached layer, to re-bake a missing layer on a non-dirty
    /// frame instead of baking empty (transparent) ops. See the native renderer.
    cached_ops: HashMap<u32, Vec<DrawOp>>,
    transform_compositor: Option<TransformCompositor>,
}

/// A frame-sized vello target (see `gpu_surface::make_target`).
fn make_target(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    gpu_surface::make_target(device, w, h, "canvas-vello-web-target")
}

impl GpuState {
    fn new(gpu: GpuSurface, renderer: Renderer, layers: Vec<TextureLayer>) -> GpuState {
        GpuState {
            gpu,
            renderer,
            image_renderer: None,
            anim: AnimTextures::new(),
            scene: VelloScene::new(),
            layers,
            layer_compositor: None,
            shape_pass: None,
            overlay: None,
            overlay_compositor: None,
            cached_layers: HashMap::new(),
            cached_ops: HashMap::new(),
            transform_compositor: None,
        }
    }

    /// (Re)bake the dirty cached layers in a `ScenePlan::Cached` backdrop into
    /// their viewport-sized textures — only on `dirty` (or first sight / after a
    /// resize dropped the texture). A `dirty: false` pan reuses the retained
    /// texture and this does nothing, so the per-frame cost is the composite, not
    /// the raster. The WebGPU/vello path is the ideal web path; this is where the
    /// pan/zoom win lands (Canvas2D is only the no-WebGPU fallback).
    fn bake_cached_layers(&mut self, layers: &[CachedRef]) {
        let (w, h) = (self.gpu.width(), self.gpu.height());
        // Overscan (see the native `render::bake_cached_layers` for the rationale):
        // bake into a texture `frac`·viewport larger per side so a pan within the
        // margin composites with no black edge. `0.0` (default) = viewport-sized.
        let frac = overscan_frac();
        let (ow, oh) = overscan_dims(w, h, frac);
        let margin = (
            frac as f64 * (w as f64) / self.gpu.scale(),
            frac as f64 * (h as f64) / self.gpu.scale(),
        );
        for layer in layers {
            let missing = !self.cached_layers.contains_key(&layer.id);
            if !(layer.dirty || missing) {
                continue;
            }
            // Retain the last DIRTY ops and re-bake from them when a missing layer
            // must bake on a non-dirty frame — otherwise the empty `dirty=false`
            // ops bake a transparent (black) layer (the "canvas black until I draw"
            // bug, e.g. after an aspect/viewport resize). See the native renderer.
            if layer.dirty {
                self.cached_ops.insert(layer.id, layer.ops.to_vec());
            }
            let Some(ops) = self.cached_ops.remove(&layer.id) else {
                continue;
            };
            if missing {
                self.cached_layers.insert(layer.id, make_target(&self.gpu.device, ow, oh));
            }
            self.scene.reset();
            encode_scene(
                &ops,
                &mut self.scene,
                Affine::scale(self.gpu.scale()) * Affine::translate(margin),
            );
            let params = RenderParams {
                base_color: Color::from_rgba8(0, 0, 0, 0),
                width: ow,
                height: oh,
                antialiasing_method: AaConfig::Area,
            };
            // Image layers bake on the dedicated `image_renderer` so the grid/ink
            // bakes can't shrink the image atlas out from under them (see the
            // `image_renderer` field). Image-less layers stay on the main renderer.
            let has_image = ops.iter().any(|op| matches!(op, DrawOp::Image { .. }));
            if has_image && self.image_renderer.is_none() {
                self.image_renderer = Renderer::new(
                    &self.gpu.device,
                    RendererOptions {
                        use_cpu: false,
                        antialiasing_support: AaSupport::area_only(),
                        num_init_threads: None,
                        pipeline_cache: None,
                    },
                )
                .ok();
            }
            // Flush any animated-image frames this bake's encode staged into
            // their override textures before rendering (see `anim.rs`).
            self.anim.apply(
                &self.gpu.device,
                &self.gpu.queue,
                &mut self.renderer,
                self.image_renderer.as_mut(),
            );
            let view = &self.cached_layers.get(&layer.id).unwrap().1;
            let renderer = match (has_image, self.image_renderer.as_mut()) {
                (true, Some(r)) => r,
                _ => &mut self.renderer,
            };
            let _ =
                renderer.render_to_texture(&self.gpu.device, &self.gpu.queue, &self.scene, view, &params);
            self.cached_ops.insert(layer.id, ops);
        }
    }

    /// Encode `ops` with vello and render them into `target` (clearing it) or,
    /// with `to_overlay`, into the separate `overlay` texture (which must
    /// exist) over a transparent base. `render_to_texture` SUBMITS its own
    /// command buffer immediately. Returns `false` if vello failed.
    fn render_vello(&mut self, ops: &[DrawOp], to_overlay: bool) -> bool {
        self.scene.reset();
        // Base transform = device scale: the author's Scene is logical; scaling
        // by dpr fills the physical-pixel surface (no retina under-fill).
        encode_scene(ops, &mut self.scene, Affine::scale(self.gpu.scale()));
        let params = RenderParams {
            base_color: Color::from_rgba8(0, 0, 0, 0),
            width: self.gpu.width(),
            height: self.gpu.height(),
            antialiasing_method: AaConfig::Area,
        };
        let view = if to_overlay { &self.overlay.as_ref().unwrap().1 } else { self.gpu.target_view() };
        // Route image-bearing content (a live-dragged media item in `rest`) to
        // the dedicated `image_renderer` — the main renderer's atlas is shrunk
        // by image-less bakes, blanking a later live image (media vanishes
        // mid-drag). Mirror of the native `render` fix + the layer-bake routing.
        let has_image = ops.iter().any(|op| matches!(op, DrawOp::Image { .. }));
        if has_image && self.image_renderer.is_none() {
            self.image_renderer = Renderer::new(
                &self.gpu.device,
                RendererOptions {
                    use_cpu: false,
                    antialiasing_support: AaSupport::area_only(),
                    num_init_threads: None,
                    pipeline_cache: None,
                },
            )
            .ok();
        }
        // Flush any animated-image frames this encode staged into their
        // override textures before rendering (see `anim.rs`).
        self.anim.apply(
            &self.gpu.device,
            &self.gpu.queue,
            &mut self.renderer,
            self.image_renderer.as_mut(),
        );
        let renderer = match (has_image, self.image_renderer.as_mut()) {
            (true, Some(r)) => r,
            _ => &mut self.renderer,
        };
        renderer.render_to_texture(&self.gpu.device, &self.gpu.queue, &self.scene, view, &params).is_ok()
    }

    /// Render `canvas_scene` and present. `gpu_surface::drive` has already
    /// refreshed the dpr and re-synced the surface to the canvas backing store
    /// (calling `resized` on a change) before this runs.
    fn render(&mut self, canvas_scene: &CanvasScene) -> bool {
        // Classify the scene (see `crate::plan`) — same hybrid instanced-backdrop
        // path as the native renderer. vello renders the content (whole scene for
        // `Vello` → `target`; only `rest` for `Hybrid` → the separate `overlay`)
        // over a transparent base; `Shapes` skips vello entirely.
        //
        // Texture layers: the scene is split at its `Texture` ops
        // (`split_segments`); only the BASE segment (the whole scene when there
        // are none) is classified here, and the texture runs are composited
        // afterwards by `composite_texture_runs` — the native renderer's model.
        let segments = split_segments(canvas_scene.ops());
        let plan = plan_scene(segments.base);
        let (content_ops, to_overlay): (Option<&[DrawOp]>, bool) = match &plan {
            ScenePlan::Vello => (Some(segments.base), false),
            ScenePlan::Hybrid { rest, .. } => {
                if self.overlay.is_none() {
                    self.overlay =
                        Some(make_target(&self.gpu.device, self.gpu.width(), self.gpu.height()));
                }
                (Some(rest), true)
            }
            ScenePlan::Shapes(_) => (None, false),
            ScenePlan::Cached { rest, layers } => {
                // A `Cached` frame must carry SOME vello submit or it won't present
                // (the "pan doesn't update until you draw" freeze). A bake (dirty /
                // first-seen layer) is such a submit; live `rest` is too. Force an
                // EMPTY `rest` through vello ONLY when neither happens — a pure
                // composite-only reuse frame. When a layer bakes (drawing re-bakes
                // the ink layer every point), an empty `rest` would be a WASTED
                // full-viewport vello pass per frame. Empty overlay isn't composited
                // (guarded by `!rest.is_empty()`).
                let any_bake = layers
                    .iter()
                    .any(|l| l.dirty || !self.cached_layers.contains_key(&l.id));
                if rest.is_empty() && any_bake {
                    (None, false)
                } else {
                    if self.overlay.is_none() {
                        self.overlay = Some(make_target(
                            &self.gpu.device,
                            self.gpu.width(),
                            self.gpu.height(),
                        ));
                    }
                    (Some(rest), true)
                }
            }
        };
        // Bake dirty cached layers into their viewport-sized textures (only
        // re-rasters on `dirty`); composited under their transforms below.
        if let ScenePlan::Cached { layers, .. } = &plan {
            self.bake_cached_layers(layers);
        }
        if let Some(ops) = content_ops {
            if !self.render_vello(ops, to_overlay) {
                return false;
            }
        }

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("canvas-vello-web-blit") });

        // Instanced shape backdrop (+ compose vello's content over it for a
        // `Hybrid` scene). Disjoint field borrows — bind shared refs to locals first.
        match &plan {
            ScenePlan::Vello => {}
            ScenePlan::Shapes(batches) => {
                if self.shape_pass.is_none() {
                    self.shape_pass = Some(ShapePass::new(&self.gpu.device));
                }
                let device = &self.gpu.device;
                let queue = &self.gpu.queue;
                let target_view = self.gpu.target_view();
                let (cw, ch) = (self.gpu.width(), self.gpu.height());
                let s = self.gpu.scale() as f32;
                self.shape_pass.as_mut().unwrap().render(
                    device, queue, &mut encoder, target_view, batches, s, cw, ch,
                );
            }
            ScenePlan::Hybrid { prefix, .. } => {
                if self.shape_pass.is_none() {
                    self.shape_pass = Some(ShapePass::new(&self.gpu.device));
                }
                if self.overlay_compositor.is_none() {
                    self.overlay_compositor = Some(crate::overlay_compositor(&self.gpu.device));
                }
                let device = &self.gpu.device;
                let queue = &self.gpu.queue;
                let target_view = self.gpu.target_view();
                let (cw, ch) = (self.gpu.width(), self.gpu.height());
                let s = self.gpu.scale() as f32;
                self.shape_pass.as_mut().unwrap().render(
                    device, queue, &mut encoder, target_view, prefix, s, cw, ch,
                );
                let overlay_view = &self.overlay.as_ref().unwrap().1;
                self.overlay_compositor.as_ref().unwrap().composite(
                    device,
                    &mut encoder,
                    overlay_view,
                    target_view,
                );
            }
            ScenePlan::Cached { layers, rest } => {
                if self.transform_compositor.is_none() {
                    self.transform_compositor = Some(TransformCompositor::new(&self.gpu.device));
                }
                if !rest.is_empty() && self.overlay_compositor.is_none() {
                    self.overlay_compositor = Some(crate::overlay_compositor(&self.gpu.device));
                }
                let device = &self.gpu.device;
                let target_view = self.gpu.target_view();
                let (cw, ch) = (self.gpu.width(), self.gpu.height());
                let s = self.gpu.scale() as f32;
                let frac = overscan_frac();
                let (ow, oh) = overscan_dims(cw, ch, frac);
                // Clear, then composite each cached layer (in order) under its
                // camera transform — one transformed quad each, no per-op work.
                crate::compose_transform::clear_to_transparent(&mut encoder, target_view);
                let tc = self.transform_compositor.as_ref().unwrap();
                for layer in layers {
                    if let Some((tex, view)) = self.cached_layers.get(&layer.id) {
                        // Compare against the OVERSCAN dims (what bake allocates).
                        if tex.width() == ow && tex.height() == oh {
                            tc.composite(
                                device, &mut encoder, view, target_view,
                                layer.transform, s, layer.alpha, cw, ch, frac,
                            );
                        }
                    }
                }
                if !rest.is_empty() {
                    let overlay_view = &self.overlay.as_ref().unwrap().1;
                    self.overlay_compositor.as_ref().unwrap().composite(
                        device,
                        &mut encoder,
                        overlay_view,
                        target_view,
                    );
                }
            }
        }

        // Texture layers (the camera) + the vector runs drawn over them, INTO
        // the same target the blit + captureStream read — so the camera is on
        // screen AND in the recording. No runs (no layers) → nothing here.
        if !segments.runs.is_empty() {
            let overlay_pending = to_overlay && content_ops.is_some();
            if !composite_texture_runs(self, &segments.runs, &mut encoder, overlay_pending) {
                return false;
            }
        }

        // Frame target → surface (straight → premultiplied; see gpu-surface).
        // Skipped on timeout/outdated/lost; the next repaint retries.
        let Some(frame) = self.gpu.begin_present(&mut encoder) else {
            return false;
        };
        self.gpu.queue.submit([encoder.finish()]);
        self.gpu.finish_present(frame);
        true
    }
}

/// Texture runs (see `crate::texture_runs`): layers via the layer compositor,
/// each run's vector ops via vello into `overlay`, composited over `target`.
impl RunHost for GpuState {
    type Enc = wgpu::CommandEncoder;

    fn composite_layers(&mut self, enc: &mut wgpu::CommandEncoder, which: &[u32]) {
        let (cw, ch) = (self.gpu.width(), self.gpu.height());
        let lc = self.layer_compositor.get_or_insert_with(|| WebLayerCompositor::new(&self.gpu.device));
        lc.composite_layers(
            &self.gpu.device, &self.gpu.queue, enc, &self.layers, which, self.gpu.target_view(),
            self.gpu.scale() as f32, cw, ch,
        );
    }

    fn render_overlay(&mut self, ops: &[DrawOp]) -> bool {
        if self.overlay.is_none() {
            self.overlay = Some(make_target(&self.gpu.device, self.gpu.width(), self.gpu.height()));
        }
        self.render_vello(ops, true)
    }

    fn composite_overlay(&mut self, enc: &mut wgpu::CommandEncoder) {
        let oc = self.overlay_compositor.get_or_insert_with(|| crate::overlay_compositor(&self.gpu.device));
        oc.composite(&self.gpu.device, enc, &self.overlay.as_ref().unwrap().1, self.gpu.target_view());
    }

    fn submit(&mut self, enc: &mut wgpu::CommandEncoder) {
        let fresh = self.gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("canvas-vello-web-blit"),
        });
        self.gpu.queue.submit([std::mem::replace(enc, fresh).finish()]);
    }
}

impl SurfaceState for GpuState {
    type Frame = CanvasScene;

    fn gpu(&self) -> &GpuSurface {
        &self.gpu
    }

    fn gpu_mut(&mut self) -> &mut GpuSurface {
        &mut self.gpu
    }

    fn render(&mut self, frame: &CanvasScene) -> bool {
        GpuState::render(self, frame)
    }

    /// The overlay and cached layer textures are sized to the old drawable:
    /// drop them (the overlay is rebuilt on demand; the app re-bakes cached
    /// layers — `dirty` — on the resize repaint).
    fn resized(&mut self) {
        self.overlay = None;
        self.cached_layers.clear();
    }
}

// The async GPU bootstrap (headless probe → build vello → claim, else Canvas2D)
// is only exercisable against a real browser + GPU, so it's covered by the
// Playwright E2E (whiteboard demo + the `__IDEALYST_FORCE_CANVAS2D` hatch), not a
// unit test (CLAUDE.md §8 "closest reachable test"). The synchronous
// `navigator.gpu` gate's detection logic is unit-tested in gpu-surface.
