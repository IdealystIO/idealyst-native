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
use crate::encode::encode_scene;
use crate::plan::{plan_scene, ScenePlan};
use crate::shape_pass::ShapePass;
use crate::web_layer::WebLayerCompositor;
use canvas_core::{paint_scene, CanvasProps, DrawOp, Scene as CanvasScene, TextureLayer};
use runtime_core::accessibility::AccessibilityProps;
use runtime_core::primitives::graphics::{GraphicsSurface, OnReadyEvent, OnResizeEvent};
use runtime_core::{Backend, Effect, RegisterExternal};

use std::cell::RefCell;
use std::rc::Rc;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use vello::kurbo::Affine;
use vello::peniko::Color;
use vello::{AaConfig, AaSupport, Renderer, RendererOptions, RenderParams, Scene as VelloScene};
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

fn build_canvas<B: Backend>(props: &Rc<CanvasProps>, backend: &mut B) -> B::Node {
    // Latest painted scene + the installed renderer, shared between the reactive
    // effect and the surface lifecycle callbacks. `render_fn` is `None` until
    // the async `on_ready` probe installs a GPU or Canvas2D renderer.
    let scene_cell: Rc<RefCell<CanvasScene>> = Rc::new(RefCell::new(CanvasScene::new()));
    let render_fn: Rc<RefCell<Option<RenderFn>>> = Rc::new(RefCell::new(None));

    // Reactive repaint, anchored in the mount scope (this is what keeps repaints
    // alive past `build_canvas` return — see [[project_flatlist_needs_component_scope]]).
    // Recomputes the scene whenever a signal the draw closure reads changes, and
    // draws if a renderer has been installed yet (the first draw is done by
    // `on_ready`, once the async probe resolves).
    let _effect = Effect::new({
        let props = props.clone();
        let scene_cell = scene_cell.clone();
        let render_fn = render_fn.clone();
        move || {
            *scene_cell.borrow_mut() = paint_scene(&props);
            repaint(&render_fn, &scene_cell);
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

    if gpu_viable {
        if let Some(gpu) = GpuState::try_new(ev, canvas.clone(), props.layers.clone()).await {
            // Self-capture works on a webgpu-context canvas via captureStream —
            // and the camera is composited INTO the canvas, so it's in the recording.
            canvas_native::publish_capture_stream(&canvas, &props);
            marker("canvas-vello: web GPU (WebGPU)");
            return gpu.into_render_fn();
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
        let scale = if ev.scale > 0.0 { ev.scale as f64 } else { 1.0 };

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            // On wasm32 `PRIMARY` is the browser's WebGPU backend.
            backends: wgpu::Backends::PRIMARY,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });

        // Headless adapter+device — `compatible_surface: None` never touches the
        // canvas, so it stays unclaimed if any of this fails.
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .ok()?;

        // vello's `flatten` shader wants f16 where the backend offers it; request
        // it when present. Take the adapter's own limits (never over-asks).
        let f16 = wgpu::Features::SHADER_F16 & adapter.features();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("canvas-vello-web-device"),
                required_features: f16,
                required_limits: adapter.limits(),
                memory_hints: wgpu::MemoryHints::default(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .ok()?;

        // Build the vello pipeline BEFORE claiming the canvas: this is the last
        // step that can fail on a too-weak GPU. If it errors, the canvas is still
        // pristine and the caller falls back to Canvas2D.
        let renderer = Renderer::new(
            &device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: AaSupport::area_only(),
                num_init_threads: None,
                pipeline_cache: None,
            },
        )
        .ok()?;

        // --- Commit: this is the only step that binds the canvas to webgpu. ---
        let surface = instance.create_surface(ev.surface).ok()?;

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
            desired_maximum_frame_latency: 2,
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
        })
    }

    fn into_render_fn(self) -> RenderFn {
        let mut state = self;
        Box::new(move |scene: &CanvasScene| state.render(scene))
    }

    fn render(&mut self, canvas_scene: &CanvasScene) {
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
        };
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
            if self
                .renderer
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
