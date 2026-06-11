//! Web (wasm32) vello renderer with a per-canvas Canvas2D fallback.
//!
//! Renders a `canvas_core::Scene` with vello over wgpu's **WebGPU** backend.
//! WebGPU is not universal, so robustness is the hard part — and two web rules
//! shape the design:
//!
//! 1. A `<canvas>` is **permanently bound to its first context type**: once
//!    `getContext("webgpu")` is called (which wgpu's `create_surface` does),
//!    `getContext("2d")` on that element returns `null` forever.
//! 2. The web backend has **no runtime node-swap for externals** — a mounted
//!    node stays.
//!
//! So we can't "register vello, swap to canvas-native on failure," and we can't
//! claim the canvas for webgpu before we know the GPU works. Instead each canvas
//! decides for itself in `on_ready` (**lazy per-canvas context selection**):
//!
//! - A **headless** adapter+device probe runs FIRST — `compatible_surface: None`,
//!   and the vello `Renderer` is built from the device too. None of this touches
//!   the canvas, so it stays unclaimed.
//! - Only once the device AND the vello pipeline are in hand do we
//!   `create_surface` (the one step that claims the canvas) and render with vello.
//! - If the probe fails (no adapter / weak GPU / `Renderer::new` error) we hand
//!   the still-unclaimed canvas to `canvas-native`'s `make_2d_rasterizer` and
//!   render identical output via Canvas2D — same element, no node-swap, never
//!   blank (CLAUDE.md §7).
//!
//! `register` also gates on `navigator.gpu` synchronously, so browsers with no
//! WebGPU at all never override the `canvas-native` handler and pay no probe.
//!
//! Texture layers (camera-in-canvas) and self-capture: the GPU path has no
//! layer compositor on web (that lives in the native-only `native_capture`
//! module), so a canvas with `layers` takes the Canvas2D path, which composites
//! them. Self-capture uses `captureStream()` on both paths (works on a
//! webgpu-context canvas) — no GPU→CPU readback, whose blocking `map`+`poll`
//! would be illegal on the wasm main thread.

use crate::compose::OverlayCompositor;
use crate::compose_transform::TransformCompositor;
use crate::encode::encode_scene;
use crate::plan::{plan_scene, CachedRef, ScenePlan};
use crate::shape_pass::ShapePass;
use crate::web_layer::WebLayerCompositor;
use canvas_core::{paint_scene, CanvasProps, DrawOp, Scene as CanvasScene, TextureLayer};
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::graphics::{GraphicsSurface, OnReadyEvent, OnResizeEvent};
use runtime_core::{Backend, Effect, RegisterExternal};

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use vello::kurbo::Affine;
use vello::peniko::Color;
use vello::{AaConfig, AaSupport, Renderer, RendererOptions, RenderParams, Scene as VelloScene};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::HtmlCanvasElement;

/// A repaint sink: given the latest logical-coordinate scene, draw a frame.
/// Either the vello GPU renderer or canvas-native's Canvas2D rasterizer; the
/// reactive effect, `on_ready`, and `on_resize` all drive it the same way.
type RenderFn = Box<dyn FnMut(&CanvasScene)>;

/// vello renders into a storage texture of this format; the blitter copies it
/// to the surface (whatever the surface's own format is).
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Register the web vello canvas renderer. Overrides `canvas-native` (registered
/// first, via inventory) only when the browser exposes WebGPU at all; the
/// per-GPU viability decision is deferred to each canvas's `on_ready`.
pub fn register<B: RegisterExternal>(backend: &mut B) {
    canvas_core::ensure_wire_serde();
    // Synchronous, deterministic gate. Absent `navigator.gpu` → leave
    // canvas-native installed; no wasted async probe. A *present* `navigator.gpu`
    // can still fail to yield an adapter (driver blocklists, VMs); that case is
    // handled per-canvas by the Canvas2D fallback in `on_ready`.
    if !webgpu_present() {
        return;
    }
    backend.register_external::<CanvasProps, _>(build_canvas);
}

/// One-line console note of which renderer engaged. Goes straight to
/// `console.log` (not the `log` facade, which the web logger may filter) so the
/// per-canvas path decision is always visible in devtools — and is what the E2E
/// asserts.
fn marker(msg: &str) {
    web_sys::console::log_1(&JsValue::from_str(msg));
}

/// Is `obj[key]` present and truthy? Read via `js_sys::Reflect` so we don't pull
/// the unstable web-sys WebGPU typings just for a truthiness check. A present
/// object (e.g. `navigator.gpu`) is truthy; `undefined`/`null`/`false` are not.
fn js_truthy_prop(obj: &JsValue, key: &str) -> bool {
    js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .map(|v| v.is_truthy())
        .unwrap_or(false)
}

/// `navigator.gpu` presence — the synchronous WebGPU availability gate.
fn webgpu_present() -> bool {
    web_sys::window()
        .map(|w| js_truthy_prop(w.navigator().as_ref(), "gpu"))
        .unwrap_or(false)
}

/// Debug-only E2E escape hatch: `window.__IDEALYST_FORCE_CANVAS2D = true` forces
/// the Canvas2D fallback so the async-bootstrap fallback branch can be exercised
/// without a blocklisted GPU. Compiles to `false` in release builds (CLAUDE.md
/// §7: dev-only markers don't survive into release).
fn force_canvas2d() -> bool {
    #[cfg(debug_assertions)]
    {
        web_sys::window()
            .map(|w| js_truthy_prop(w.as_ref(), "__IDEALYST_FORCE_CANVAS2D"))
            .unwrap_or(false)
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

/// Desktop devices top out at dpr 2.0 (Retina); above that we're on a high-dpi
/// mobile device, where the physical backing store — and every full-viewport
/// render pass over it — is 6–12× a dpr-1 surface, so the canvas goes
/// fill-rate-bound on the mobile GPU. Cap the mobile dpr to trade a little
/// sharpness for a large pixel-count cut; desktops (≤ 2.0) keep their full dpr.
///
/// MUST stay identical to the backing-store clamp in the web backend's graphics
/// primitive (`backend/web/.../graphics.rs::effective_dpr`) — this scales the
/// author scene, that sizes the surface; if they disagree the scene under-/
/// over-fills the surface (the retina mis-fill the doc below warns about).
const DPR_DESKTOP_MAX: f64 = 2.0;
const DPR_MOBILE_CAP: f64 = 1.5;

/// Device-pixel ratio from `window.devicePixelRatio`, clamped on mobile (see
/// [`DPR_DESKTOP_MAX`]). The web graphics primitive sizes the canvas backing
/// store to css × dpr but reports `OnReadyEvent.scale == 1.0` ("size is
/// physical, no separate scale"), so the GPU renderer derives the dpr here —
/// matching the Canvas2D path — to scale the logical author scene up to the
/// physical surface. Without it the scene fills only the top-left 1/dpr.
fn web_dpr() -> f64 {
    let raw = web_sys::window()
        .map(|w| w.device_pixel_ratio())
        .filter(|d| *d > 0.0)
        .unwrap_or(1.0);
    if raw > DPR_DESKTOP_MAX {
        DPR_MOBILE_CAP
    } else {
        raw
    }
}

fn build_canvas<B: Backend>(props: &Rc<CanvasProps>, backend: &mut B) -> B::Node {
    // Latest painted scene + the installed renderer, shared between the reactive
    // effect and the surface lifecycle callbacks. `render_fn` is `None` until
    // the async `on_ready` probe installs a GPU or Canvas2D renderer.
    let scene_cell: Rc<RefCell<CanvasScene>> = Rc::new(RefCell::new(CanvasScene::new()));
    let render_fn: Rc<RefCell<Option<RenderFn>>> = Rc::new(RefCell::new(None));
    // Whether a `requestAnimationFrame` render is already queued. The reactive
    // effect can fire many times per displayed frame (pan/zoom pointer/wheel
    // events arrive in dense bursts), but we only need ONE render per frame.
    let frame_pending: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    // Reactive repaint, anchored in the mount scope (this is what keeps repaints
    // alive past `build_canvas` return — see [[project_flatlist_needs_component_scope]]).
    // Recomputes the scene whenever a signal the draw closure reads changes, then
    // schedules ONE rAF-aligned render (the first draw is done by `on_ready`, once
    // the async probe resolves). Coalescing to rAF is essential on web: WebGPU's
    // present is non-blocking, so rendering synchronously per input event
    // over-submits to the swapchain (250+ fps) until it backpressure-stalls. One
    // render per animation frame caps it at the display refresh.
    let _effect = Effect::new({
        let props = props.clone();
        let scene_cell = scene_cell.clone();
        let render_fn = render_fn.clone();
        let frame_pending = frame_pending.clone();
        move || {
            *scene_cell.borrow_mut() = paint_scene(&props);
            schedule_repaint(&render_fn, &scene_cell, &frame_pending);
        }
    });

    let on_ready = {
        let scene_cell = scene_cell.clone();
        let render_fn = render_fn.clone();
        let props = props.clone();
        move |ev: OnReadyEvent| {
            // Acquire the GPU asynchronously — blocking is illegal on the wasm
            // main thread. A fresh `on_ready` can follow an `on_lost`, so each
            // run does its own probe and reinstalls `render_fn`.
            let scene_cell = scene_cell.clone();
            let render_fn = render_fn.clone();
            let props = props.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let f = build_render_fn(ev, props).await;
                *render_fn.borrow_mut() = Some(f);
                // First paint with whatever renderer we ended up with.
                repaint(&render_fn, &scene_cell);
            });
        }
    };

    let on_resize = {
        let scene_cell = scene_cell.clone();
        let render_fn = render_fn.clone();
        // The renderer re-reads the (already-resized) canvas backing store each
        // frame, so a resize just needs to trigger a repaint.
        move |_ev: OnResizeEvent| repaint(&render_fn, &scene_cell)
    };

    let on_lost = {
        let render_fn = render_fn.clone();
        // Drop the renderer (and its GPU surface). A fresh `on_ready` re-probes.
        move || *render_fn.borrow_mut() = None
    };

    backend.create_graphics(
        Box::new(on_ready),
        Box::new(on_resize),
        Box::new(on_lost),
        &AccessibilityProps::default(),
    )
}

/// Queue a render for the next animation frame, coalescing: if one is already
/// pending, this is a no-op, so a burst of reactive updates within a frame
/// collapses to a single render of the LATEST scene. The rAF callback clears the
/// flag and repaints. This is what paces web rendering to the display refresh
/// (WebGPU's present doesn't block, so it must be paced explicitly).
fn schedule_repaint(
    render_fn: &Rc<RefCell<Option<RenderFn>>>,
    scene_cell: &Rc<RefCell<CanvasScene>>,
    frame_pending: &Rc<Cell<bool>>,
) {
    if frame_pending.replace(true) {
        return; // a frame is already queued — fold into it
    }
    let Some(window) = web_sys::window() else {
        frame_pending.set(false);
        return;
    };
    let render_fn = render_fn.clone();
    let scene_cell = scene_cell.clone();
    // Clone for the closure; keep the `&Rc` param for the error fallback below.
    let pending_cb = frame_pending.clone();
    // `once_into_js` keeps the closure alive until JS invokes it once, then drops
    // it — no manual `Closure` lifetime management for a one-shot rAF.
    let cb = Closure::once_into_js(move || {
        pending_cb.set(false);
        repaint(&render_fn, &scene_cell);
    });
    if window.request_animation_frame(cb.unchecked_ref()).is_err() {
        frame_pending.set(false);
    }
}

/// Run the installed renderer (if any) against the latest scene. Takes the
/// closure out of the `RefCell` across the call so a reentrant signal write in
/// the draw path can't double-borrow.
fn repaint(render_fn: &Rc<RefCell<Option<RenderFn>>>, scene_cell: &Rc<RefCell<CanvasScene>>) {
    let mut taken = render_fn.borrow_mut().take();
    if let Some(f) = taken.as_mut() {
        f(&scene_cell.borrow());
    }
    // Put it back unless `on_lost` cleared the slot while we rendered.
    let mut slot = render_fn.borrow_mut();
    if slot.is_none() {
        *slot = taken;
    }
}

/// Decide the renderer for one canvas: vello GPU when WebGPU is viable and the
/// canvas has no texture layers, else canvas-native's Canvas2D rasterizer on the
/// same (still-unclaimed) element.
async fn build_render_fn(ev: OnReadyEvent, props: Rc<CanvasProps>) -> RenderFn {
    let canvas = match canvas_from_surface(&ev.surface) {
        Some(c) => c,
        // Should never happen on web (the graphics surface IS a canvas); degrade
        // to a no-op rather than panic in an async task.
        None => return Box::new(|_| {}),
    };

    // Texture layers (the camera) are now composited on the GPU path too (see
    // `web_layer::WebLayerCompositor`), so a layered canvas no longer forces
    // Canvas2D — it stays on WebGPU/vello (the instanced backdrop included).
    let gpu_viable = !force_canvas2d();
    if !gpu_viable {
        marker("canvas-vello: Canvas2D forced (__IDEALYST_FORCE_CANVAS2D set)");
    }

    if gpu_viable {
        if let Some(gpu) = GpuState::try_new(ev, canvas.clone(), props.layers.clone()).await {
            // Self-capture works on a webgpu-context canvas via captureStream —
            // and the camera is composited INTO the canvas, so it's in the recording.
            // Manual capture mode: `tick()` the driver after each present, because
            // a WebGPU swapchain present doesn't reliably trigger the browser's
            // auto-capture timer (choppy recordings). See `publish_capture_stream`.
            let capture = canvas_native::publish_capture_stream(&canvas, &props);
            marker("canvas-vello: web GPU (WebGPU)");
            let mut render = gpu.into_render_fn();
            return Box::new(move |scene: &CanvasScene| {
                render(scene);
                if let Some(c) = &capture {
                    c.tick();
                }
            });
        }
    }

    marker("canvas-vello: web GPU unavailable — Canvas2D fallback");
    canvas_native::make_2d_rasterizer(canvas, &props)
}

/// Reconstruct the graphics primitive's `<canvas>` from its
/// `WebCanvasWindowHandle`, so the Canvas2D fallback (and the GPU path's resize
/// size-read) can reach the element.
fn canvas_from_surface(surface: &GraphicsSurface) -> Option<HtmlCanvasElement> {
    let handle = surface.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::WebCanvas(h) => {
            // SAFETY: the web backend's surface provider builds this handle from
            // the canvas's `&JsValue` (see backend-web graphics primitive). The
            // `GraphicsSurface` `Arc` (held by the live `OnReadyEvent`) keeps the
            // canvas alive for this call, and wasm32 is single-threaded, so the
            // pointer is valid and unaliased. We clone out an owned handle.
            let js: &JsValue = unsafe { &*(h.obj.as_ptr() as *const JsValue) };
            js.dyn_ref::<HtmlCanvasElement>().cloned()
        }
        _ => None,
    }
}

// ============================================================================
// GPU render state (web)
// ============================================================================

struct GpuState {
    /// The graphics primitive keeps this canvas's backing store synced to the
    /// CSS box × dpr; `render` re-reads it to reconfigure on resize.
    canvas: HtmlCanvasElement,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    /// Second vello renderer used ONLY to bake cached layers containing images.
    /// vello keeps one persistent image atlas per `Renderer`, resized to fit each
    /// baked scene's images; baking an image-less layer (grid/ink) through the
    /// same renderer shrinks the atlas to 1×1, and a later image bake resizes it
    /// back WITHOUT re-uploading the cached image → the image renders blank.
    /// Image layers on their own renderer keep their atlas intact. Lazily built.
    image_renderer: Option<Renderer>,
    scene: VelloScene,
    /// Intermediate Rgba8Unorm storage texture vello renders into (the surface
    /// can't be a compute storage target); blitted to the surface each frame.
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    blitter: wgpu::util::TextureBlitter,
    /// Device pixel ratio: the author's `Scene` is logical, so this base
    /// transform makes it fill the physical-pixel surface (no retina under-fill).
    scale: f64,
    /// Texture layers (the camera) composited over the scene each frame via
    /// [`WebLayerCompositor`]. Empty when the canvas has no layers.
    layers: Vec<TextureLayer>,
    layer_compositor: Option<WebLayerCompositor>,
    /// Instanced analytic-shape pass for a leading shape backdrop (the hybrid
    /// path), built lazily the first time a shape-led scene is rendered.
    shape_pass: Option<ShapePass>,
    /// Secondary target vello renders a HYBRID scene's `rest` into (over a
    /// transparent base); [`OverlayCompositor`] lays it over the instanced
    /// backdrop in `target`. Lazily created, invalidated on resize.
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

impl GpuState {
    /// Probe the GPU **without claiming the canvas**, build the vello pipeline,
    /// and only then `create_surface` (the single canvas-claiming step). Returns
    /// `None` — leaving the canvas pristine for the Canvas2D fallback — when no
    /// adapter/device is available or the GPU is too weak for vello's pipeline.
    async fn try_new(
        ev: OnReadyEvent,
        canvas: HtmlCanvasElement,
        layers: Vec<TextureLayer>,
    ) -> Option<GpuState> {
        let (w, h) = (ev.size.0.max(1), ev.size.1.max(1));
        // The web graphics primitive reports `size` as PHYSICAL (css × dpr) with
        // `ev.scale == 1.0` ("size is physical, no separate scale" contract). So
        // the device-pixel ratio has to be derived here — exactly like the
        // Canvas2D path (`canvas-native` web `render_scene`) — and used as the
        // base transform to scale the LOGICAL author scene up to the physical
        // surface. Using `ev.scale` (1.0) renders the scene at 1× into the
        // dpr-sized target, filling only the top-left 1/dpr (the retina bug).
        let scale = web_dpr();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            // On wasm32 `PRIMARY` is the browser's WebGPU backend.
            backends: wgpu::Backends::PRIMARY,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });

        // Headless adapter+device — `compatible_surface: None` never touches the
        // canvas, so it stays unclaimed if any of this fails. TEMP: each failure
        // logs WHY we fall back to Canvas2D (remove the markers once diagnosed).
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
        {
            Ok(a) => a,
            Err(e) => {
                marker(&format!("canvas-vello: WebGPU probe — no adapter ({e:?})"));
                return None;
            }
        };
        let info = adapter.get_info();
        marker(&format!(
            "canvas-vello: WebGPU adapter ok — {:?} / {} (backend {:?})",
            info.device_type, info.name, info.backend
        ));

        // vello's `flatten` shader wants f16 where the backend offers it; request
        // it when present. Take the adapter's own limits (never over-asks).
        let f16 = wgpu::Features::SHADER_F16 & adapter.features();
        let (device, queue) = match adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("canvas-vello-web-device"),
                required_features: f16,
                required_limits: adapter.limits(),
                memory_hints: wgpu::MemoryHints::default(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                trace: wgpu::Trace::Off,
            })
            .await
        {
            Ok(dq) => dq,
            Err(e) => {
                marker(&format!("canvas-vello: WebGPU probe — request_device failed ({e:?})"));
                return None;
            }
        };

        // Build the vello pipeline BEFORE claiming the canvas: this is the last
        // step that can fail on a too-weak GPU. If it errors, the canvas is still
        // pristine and the caller falls back to Canvas2D.
        let renderer = match Renderer::new(
            &device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: None,
                pipeline_cache: None,
            },
        ) {
            Ok(r) => r,
            Err(e) => {
                marker(&format!("canvas-vello: WebGPU probe — vello Renderer::new failed ({e:?})"));
                return None;
            }
        };
        let image_renderer = None;

        // --- Commit: this is the only step that binds the canvas to webgpu. ---
        let surface = match instance.create_surface(ev.surface) {
            Ok(s) => s,
            Err(e) => {
                marker(&format!("canvas-vello: WebGPU probe — create_surface failed ({e:?})"));
                return None;
            }
        };

        let caps = surface.get_capabilities(&adapter);
        // Prefer a NON-sRGB surface format: vello writes already-sRGB-encoded
        // bytes into the linear Rgba8Unorm target and the blit is a straight
        // copy, so an sRGB surface would gamma-encode them again and wash the
        // colors out. Fall back to the default if no linear format is offered.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: w,
            height: h,
            present_mode: wgpu::PresentMode::AutoVsync,
            // 1, not 2: this is a direct-manipulation surface (draw / pan / pinch
            // track a finger), where input-to-photon LATENCY matters far more than
            // pipelining throughput. A 2-frame queue adds a whole extra frame of lag
            // (~22ms at 45fps) on top of the render time, which on mobile reads as
            // the canvas "rubber-banding" / lagging behind the finger on a reversal.
            desired_maximum_frame_latency: 1,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let (target, target_view) = make_target(&device, w, h);
        let blitter = wgpu::util::TextureBlitter::new(&device, format);

        Some(GpuState {
            canvas,
            device,
            queue,
            surface,
            config,
            renderer,
            image_renderer,
            scene: VelloScene::new(),
            target,
            target_view,
            blitter,
            scale,
            layers,
            layer_compositor: None,
            shape_pass: None,
            overlay: None,
            overlay_compositor: None,
            cached_layers: HashMap::new(),
            cached_ops: HashMap::new(),
            transform_compositor: None,
        })
    }

    fn into_render_fn(self) -> RenderFn {
        let mut state = self;
        Box::new(move |scene: &CanvasScene| state.render(scene))
    }

    /// (Re)bake the dirty cached layers in a `ScenePlan::Cached` backdrop into
    /// their viewport-sized textures — only on `dirty` (or first sight / after a
    /// resize dropped the texture). A `dirty: false` pan reuses the retained
    /// texture and this does nothing, so the per-frame cost is the composite, not
    /// the raster. The WebGPU/vello path is the ideal web path; this is where the
    /// pan/zoom win lands (Canvas2D is only the no-WebGPU fallback).
    fn bake_cached_layers(&mut self, layers: &[CachedRef]) {
        let (w, h) = (self.config.width, self.config.height);
        // Overscan (see the native `render::bake_cached_layers` for the rationale):
        // bake into a texture `frac`·viewport larger per side so a pan within the
        // margin composites with no black edge. `0.0` (default) = viewport-sized.
        let frac = overscan_frac();
        let (ow, oh) = overscan_dims(w, h, frac);
        let margin = (
            frac as f64 * (w as f64) / self.scale,
            frac as f64 * (h as f64) / self.scale,
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
                self.cached_layers.insert(layer.id, make_target(&self.device, ow, oh));
            }
            self.scene.reset();
            encode_scene(
                &ops,
                &mut self.scene,
                Affine::scale(self.scale) * Affine::translate(margin),
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
                    &self.device,
                    RendererOptions {
                        use_cpu: false,
                        antialiasing_support: AaSupport::area_only(),
                        num_init_threads: None,
                        pipeline_cache: None,
                    },
                )
                .ok();
            }
            let view = &self.cached_layers.get(&layer.id).unwrap().1;
            let renderer = match (has_image, self.image_renderer.as_mut()) {
                (true, Some(r)) => r,
                _ => &mut self.renderer,
            };
            let _ =
                renderer.render_to_texture(&self.device, &self.queue, &self.scene, view, &params);
            self.cached_ops.insert(layer.id, ops);
        }
    }

    fn render(&mut self, canvas_scene: &CanvasScene) {
        // Refresh the device-pixel ratio each frame (it can change when the window
        // moves between monitors or the page zooms); the backing-store size below
        // tracks it via the graphics primitive, and the base transform uses it.
        self.scale = web_dpr();

        // Live resize: the graphics primitive keeps the canvas backing store at
        // box × dpr, so reconfigure the surface + target when it changes (web has
        // no separate swapchain-size signal we need to thread through).
        let cw = self.canvas.width().max(1);
        let ch = self.canvas.height().max(1);
        if cw != self.config.width || ch != self.config.height {
            self.config.width = cw;
            self.config.height = ch;
            self.surface.configure(&self.device, &self.config);
            let (target, target_view) = make_target(&self.device, cw, ch);
            self.target = target;
            self.target_view = target_view;
            self.overlay = None; // sized to target; rebuilt on demand
            // Cached layer textures are viewport-sized — drop stale-size rasters;
            // the app re-bakes (`dirty`) on the resize repaint.
            self.cached_layers.clear();
        }

        // Classify the scene (see `crate::plan`) — same hybrid instanced-backdrop
        // path as the native renderer. vello renders the content (whole scene for
        // `Vello` → `target`; only `rest` for `Hybrid` → the separate `overlay`)
        // over a transparent base; `Shapes` skips vello entirely.
        let plan = plan_scene(canvas_scene.ops());
        let (content_ops, to_overlay): (Option<&[DrawOp]>, bool) = match &plan {
            ScenePlan::Vello => (Some(canvas_scene.ops()), false),
            ScenePlan::Hybrid { rest, .. } => {
                if self.overlay.is_none() {
                    self.overlay =
                        Some(make_target(&self.device, self.config.width, self.config.height));
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
                            &self.device,
                            self.config.width,
                            self.config.height,
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
            self.scene.reset();
            // Base transform = device scale: the author's Scene is logical; scaling
            // by dpr fills the physical-pixel surface (no retina under-fill).
            encode_scene(ops, &mut self.scene, Affine::scale(self.scale));
            let params = RenderParams {
                base_color: Color::from_rgba8(0, 0, 0, 0),
                width: self.config.width,
                height: self.config.height,
                antialiasing_method: AaConfig::Area,
            };
            let view = if to_overlay { &self.overlay.as_ref().unwrap().1 } else { &self.target_view };
            // Route image-bearing content (a live-dragged media item in `rest`) to
            // the dedicated `image_renderer` — the main renderer's atlas is shrunk
            // by image-less bakes, blanking a later live image (media vanishes
            // mid-drag). Mirror of the native `render` fix + the layer-bake routing.
            let has_image = ops.iter().any(|op| matches!(op, DrawOp::Image { .. }));
            if has_image && self.image_renderer.is_none() {
                self.image_renderer = Renderer::new(
                    &self.device,
                    RendererOptions {
                        use_cpu: false,
                        antialiasing_support: AaSupport::area_only(),
                        num_init_threads: None,
                        pipeline_cache: None,
                    },
                )
                .ok();
            }
            let renderer = match (has_image, self.image_renderer.as_mut()) {
                (true, Some(r)) => r,
                _ => &mut self.renderer,
            };
            if renderer
                .render_to_texture(&self.device, &self.queue, &self.scene, view, &params)
                .is_err()
            {
                return;
            }
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                t
            }
            // Skip the frame on timeout/outdated/lost; the next repaint retries.
            _ => return,
        };
        let surface_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("canvas-vello-web-blit") });

        // Instanced shape backdrop (+ compose vello's content over it for a
        // `Hybrid` scene). Disjoint field borrows — bind shared refs to locals first.
        match &plan {
            ScenePlan::Vello => {}
            ScenePlan::Shapes(batches) => {
                if self.shape_pass.is_none() {
                    self.shape_pass = Some(ShapePass::new(&self.device));
                }
                let device = &self.device;
                let queue = &self.queue;
                let target_view = &self.target_view;
                let (cw, ch) = (self.config.width, self.config.height);
                let s = self.scale as f32;
                self.shape_pass.as_mut().unwrap().render(
                    device, queue, &mut encoder, target_view, batches, s, cw, ch,
                );
            }
            ScenePlan::Hybrid { prefix, .. } => {
                if self.shape_pass.is_none() {
                    self.shape_pass = Some(ShapePass::new(&self.device));
                }
                if self.overlay_compositor.is_none() {
                    self.overlay_compositor = Some(OverlayCompositor::new(&self.device));
                }
                let device = &self.device;
                let queue = &self.queue;
                let target_view = &self.target_view;
                let (cw, ch) = (self.config.width, self.config.height);
                let s = self.scale as f32;
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
                    self.transform_compositor = Some(TransformCompositor::new(&self.device));
                }
                if !rest.is_empty() && self.overlay_compositor.is_none() {
                    self.overlay_compositor = Some(OverlayCompositor::new(&self.device));
                }
                let device = &self.device;
                let target_view = &self.target_view;
                let (cw, ch) = (self.config.width, self.config.height);
                let s = self.scale as f32;
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

        // Composite the texture layers (camera) over the scene, INTO the same
        // target the blit + captureStream read — so the camera is on screen AND in
        // the recording, while the dots backdrop above stayed GPU-instanced.
        if !self.layers.is_empty() {
            if self.layer_compositor.is_none() {
                self.layer_compositor = Some(WebLayerCompositor::new(&self.device));
            }
            let device = &self.device;
            let queue = &self.queue;
            let target_view = &self.target_view;
            let (cw, ch) = (self.config.width, self.config.height);
            let s = self.scale as f32;
            self.layer_compositor.as_mut().unwrap().composite_layers(
                device, queue, &mut encoder, &self.layers, target_view, s, cw, ch,
            );
        }

        self.blitter.copy(&self.device, &mut encoder, &self.target_view, &surface_view);
        self.queue.submit([encoder.finish()]);
        frame.present();
    }
}

/// Intermediate Rgba8Unorm target vello compute-writes into, then the blitter
/// samples. `RENDER_ATTACHMENT` so the instanced [`ShapePass`], the hybrid
/// [`OverlayCompositor`], and the [`WebLayerCompositor`] can draw into it; also
/// the secondary `overlay` target (vello content for a hybrid scene). No
/// COPY_SRC — web has no GPU readback (capture uses captureStream).
/// Overscan margin per side (fraction of viewport) for cached layers, from
/// `OVERSCAN_FRAC` (default `0.0`). Mirror of `render::overscan_frac` (the two
/// render paths are cfg-exclusive, so the helper is duplicated rather than shared).
fn overscan_frac() -> f32 {
    thread_local! {
        static FRAC: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) };
    }
    FRAC.with(|c| match c.get() {
        Some(v) => v,
        None => {
            let v = std::env::var("OVERSCAN_FRAC")
                .ok()
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            c.set(Some(v));
            v
        }
    })
}

/// Overscanned device dims for a `(w, h)` viewport. `frac == 0` → `(w, h)`.
fn overscan_dims(w: u32, h: u32, frac: f32) -> (u32, u32) {
    if frac <= 0.0 {
        return (w, h);
    }
    let scale = 1.0 + 2.0 * frac;
    (((w as f32) * scale).round() as u32, ((h as f32) * scale).round() as u32)
}

fn make_target(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("canvas-vello-web-target"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

// The async GPU bootstrap (headless-probe-before-claim → webgpu vs Canvas2D) is
// only exercisable against a real browser + GPU, so it's covered by the
// Playwright E2E (whiteboard demo + the `__IDEALYST_FORCE_CANVAS2D` hatch), not a
// unit test (CLAUDE.md §8 "closest reachable test"). What IS unit-testable is the
// synchronous registration gate's detection logic — that `register` keys off a
// truthy `navigator.gpu`. We test the underlying `js_truthy_prop` against
// synthetic objects so it's deterministic regardless of the test browser's own
// WebGPU support. Runs under `wasm-pack test`.
#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::js_truthy_prop;
    use wasm_bindgen::JsValue;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    // `js_sys::Object` / `Reflect` need a JS host — run in the browser, not node.
    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn truthy_prop_gates_on_presence() {
        let obj = js_sys::Object::new();
        // Absent property (the "no WebGPU" browser) → gate is false.
        assert!(!js_truthy_prop(obj.as_ref(), "gpu"));
        // Present object (a real `navigator.gpu`) → truthy → gate is true.
        js_sys::Reflect::set(&obj, &JsValue::from_str("gpu"), js_sys::Object::new().as_ref())
            .unwrap();
        assert!(js_truthy_prop(obj.as_ref(), "gpu"));
        // A `false` value (e.g. an unset escape-hatch flag) is not truthy.
        js_sys::Reflect::set(&obj, &JsValue::from_str("flag"), &JsValue::FALSE).unwrap();
        assert!(!js_truthy_prop(obj.as_ref(), "flag"));
    }
}
