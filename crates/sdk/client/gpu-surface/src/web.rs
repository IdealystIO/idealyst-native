//! Web (wasm32) surface bring-up and mount lifecycle.
//!
//! Two browser rules shape everything here:
//!
//! 1. A `<canvas>` is **permanently bound to its first context type**. Once
//!    `getContext("webgpu")` (or `"webgl2"`) runs — which wgpu's
//!    `create_surface` does — every other context type returns `null` forever.
//! 2. The web backend can't swap a mounted node, so a renderer can't "try GPU,
//!    then remount as Canvas2D".
//!
//! So bring-up is split into **probe** and **claim**, and the caller decides
//! what happens between them:
//!
//! - [`WebGpuProbe::run`] acquires a WebGPU adapter + device headlessly
//!   (`compatible_surface: None`), never touching the canvas. The caller can
//!   build its pipelines on [`WebGpuProbe::device`] (vello's `Renderer::new` is
//!   the last step that can fail on a weak GPU) and only then
//!   [`WebGpuProbe::claim`]. If anything fails first, the canvas is still
//!   pristine for a Canvas2D fallback.
//! - [`claim_webgl`] (feature `webgl`) is the WebGL2 path. WebGL has no
//!   headless adapter — wgpu only exposes a GL adapter for a surface — so it
//!   claims the canvas for `"webgl2"` FIRST and then requests the adapter.
//!   There is no going back to Canvas2D after it runs.
//!
//! Rendering is paced to `requestAnimationFrame`: WebGPU's present doesn't
//! block, so rendering synchronously per input event over-submits (250+ fps)
//! until the swapchain back-pressures. The reactive effect re-paints on every
//! change; the render runs once per displayed frame with the latest paint.

use crate::config::{check_adapter, choose_alpha_mode, choose_surface_format, AdapterFacts};
use crate::present::{surface_wants_premultiplied, PresentBlit};
use crate::{make_target, FrameAlpha, Requirements};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::graphics::{GraphicsSurface, OnReadyEvent, OnResizeEvent};
use runtime_vocabulary::caps::GraphicsOps;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::HtmlCanvasElement;

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

/// One-line `console.log` note of which path engaged. Goes straight to the
/// console (not the `log` facade, which the web logger may filter) so the
/// per-view decision is always visible in devtools — and is what E2E asserts.
pub fn marker(msg: &str) {
    web_sys::console::log_1(&JsValue::from_str(msg));
}

/// Is `obj[key]` present and truthy? Read via `js_sys::Reflect` so no unstable
/// WebGPU typings are needed just for a presence check.
fn js_truthy_prop(obj: &JsValue, key: &str) -> bool {
    js_sys::Reflect::get(obj, &JsValue::from_str(key)).map(|v| v.is_truthy()).unwrap_or(false)
}

/// `navigator.gpu` presence — the synchronous WebGPU availability gate. A
/// present `navigator.gpu` can still fail to yield an adapter (driver
/// blocklists, VMs); [`WebGpuProbe::run`] is the real test.
pub fn webgpu_present() -> bool {
    web_sys::window().map(|w| js_truthy_prop(w.navigator().as_ref(), "gpu")).unwrap_or(false)
}

/// Debug-only E2E escape hatch: `window[name]` truthy. Compiles to `false` in
/// release builds (CLAUDE.md §7: dev-only markers don't survive into release).
pub fn debug_flag(name: &str) -> bool {
    #[cfg(debug_assertions)]
    {
        web_sys::window().map(|w| js_truthy_prop(w.as_ref(), name)).unwrap_or(false)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = name;
        false
    }
}

/// Desktops top out at dpr 2.0; above that we're on a high-dpi phone, where a
/// full-viewport render pass over a 6–12× backing store goes fill-rate-bound.
/// Mobile dpr is capped to trade a little sharpness for a large pixel cut.
///
/// MUST stay identical to the backing-store clamp in the web backend's
/// graphics primitive (`backend/web/.../graphics.rs::effective_dpr`) — this
/// scales the author's content, that sizes the surface; if they disagree the
/// content under-/over-fills the surface.
const DPR_DESKTOP_MAX: f64 = 2.0;
const DPR_MOBILE_CAP: f64 = 1.5;

/// Device-pixel ratio from `window.devicePixelRatio`, clamped on mobile. The
/// web graphics primitive sizes the backing store to css × dpr but reports
/// `OnReadyEvent.scale == 1.0`, so renderers derive the dpr here.
pub fn web_dpr() -> f64 {
    let raw = web_sys::window().map(|w| w.device_pixel_ratio()).filter(|d| *d > 0.0).unwrap_or(1.0);
    if raw > DPR_DESKTOP_MAX {
        DPR_MOBILE_CAP
    } else {
        raw
    }
}

/// Reconstruct the graphics primitive's `<canvas>` from its window handle.
///
/// backend-web hands out raw-window-handle's id form (`WebWindowHandle`): the
/// canvas carries `data-raw-handle="<id>"`, looked up exactly as wgpu's
/// `create_surface` looks it up (both its WebGPU and WebGL paths). A provider
/// holding a wasm-bindgen value may use `WebCanvasWindowHandle` instead.
pub fn canvas_from_surface(surface: &GraphicsSurface) -> Option<HtmlCanvasElement> {
    let handle = surface.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Web(h) => web_sys::window()?
            .document()?
            .query_selector(&format!("[data-raw-handle=\"{}\"]", h.id))
            .ok()??
            .dyn_into::<HtmlCanvasElement>()
            .ok(),
        RawWindowHandle::WebCanvas(h) => {
            // SAFETY: raw-window-handle defines `obj` as a pointer to a
            // wasm-bindgen `JsValue` holding the canvas. The `GraphicsSurface`
            // `Arc` keeps the provider (and the value) alive for this call,
            // and wasm32 is single-threaded. We clone out an owned handle.
            let js: &JsValue = unsafe { &*(h.obj.as_ptr() as *const JsValue) };
            js.dyn_ref::<HtmlCanvasElement>().cloned()
        }
        _ => None,
    }
}

/// Which browser API a [`GpuSurface`] renders through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuBackend {
    WebGpu,
    WebGl2,
}

impl GpuBackend {
    pub fn name(self) -> &'static str {
        match self {
            GpuBackend::WebGpu => "WebGPU",
            GpuBackend::WebGl2 => "WebGL2",
        }
    }
}

/// A WebGPU adapter + device acquired WITHOUT claiming any canvas.
pub struct WebGpuProbe {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    label: &'static str,
}

impl WebGpuProbe {
    /// Acquire a WebGPU adapter and device meeting `req`, headlessly. `None`
    /// (with a console marker naming why) when WebGPU is absent or unusable.
    pub async fn run(req: Requirements, label: &'static str) -> Option<WebGpuProbe> {
        if !webgpu_present() {
            marker(&format!("{label}: WebGPU absent (no navigator.gpu)"));
            return None;
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });
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
                marker(&format!("{label}: WebGPU probe — no adapter ({e:?})"));
                return None;
            }
        };
        if let Err(why) = check_adapter(AdapterFacts::of(&adapter), req) {
            marker(&format!("{label}: WebGPU adapter {why}"));
            return None;
        }
        let (device, queue) = match request_device(&adapter, label).await {
            Ok(dq) => dq,
            Err(e) => {
                marker(&format!("{label}: WebGPU probe — request_device failed ({e:?})"));
                return None;
            }
        };
        Some(WebGpuProbe { instance, adapter, device, queue, label })
    }

    /// Claim the event's `<canvas>` for WebGPU and configure it. This is the
    /// one step that binds the canvas; call it only once everything that can
    /// fail on this device has succeeded.
    pub fn claim(self, ev: OnReadyEvent, frame_alpha: FrameAlpha) -> Option<GpuSurface> {
        let label = self.label;
        let canvas = ev.surface().and_then(canvas_from_surface)?;
        let surface_target = ev.into_surface()?;
        let surface = match self.instance.create_surface(surface_target) {
            Ok(s) => s,
            Err(e) => {
                marker(&format!("{label}: WebGPU create_surface failed ({e:?})"));
                return None;
            }
        };
        let caps = surface.get_capabilities(&self.adapter);
        // wgpu's WebGPU backend under-reports alpha modes (only `Opaque`); every
        // GPUCanvasContext supports `premultiplied` per spec.
        let alpha_mode = choose_alpha_mode(&caps.alpha_modes, true);
        GpuSurface::configure(
            self.adapter,
            self.device,
            self.queue,
            surface,
            canvas,
            &caps.formats,
            alpha_mode,
            frame_alpha,
            GpuBackend::WebGpu,
            label,
        )
    }
}

/// Claim the event's `<canvas>` for **WebGL2** and bring up a device on it.
/// Claims FIRST (WebGL has no headless adapter), so a `None` here leaves a
/// canvas that can no longer take any other context: only call it as the last
/// resort. Requires the crate's `webgl` feature; without it this returns
/// `None` without touching the canvas.
pub async fn claim_webgl(
    ev: OnReadyEvent,
    frame_alpha: FrameAlpha,
    label: &'static str,
) -> Option<GpuSurface> {
    #[cfg(not(feature = "webgl"))]
    {
        let _ = (ev, frame_alpha);
        marker(&format!("{label}: WebGL2 fallback not compiled in (gpu-surface feature `webgl`)"));
        None
    }
    #[cfg(feature = "webgl")]
    {
        let canvas = ev.surface().and_then(canvas_from_surface)?;
        let surface_target = ev.into_surface()?;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::GL,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });
        // --- Commit: binds the canvas to webgl2. ---
        let surface = match instance.create_surface(surface_target) {
            Ok(s) => s,
            Err(e) => {
                marker(&format!("{label}: WebGL2 create_surface failed ({e:?})"));
                return None;
            }
        };
        let adapter = match instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await
        {
            Ok(a) => a,
            Err(e) => {
                marker(&format!("{label}: WebGL2 — no adapter ({e:?})"));
                return None;
            }
        };
        let (device, queue) = match request_device(&adapter, label).await {
            Ok(dq) => dq,
            Err(e) => {
                marker(&format!("{label}: WebGL2 request_device failed ({e:?})"));
                return None;
            }
        };
        let caps = surface.get_capabilities(&adapter);
        let alpha_mode = choose_alpha_mode(&caps.alpha_modes, false);
        GpuSurface::configure(
            adapter,
            device,
            queue,
            surface,
            canvas,
            &caps.formats,
            alpha_mode,
            frame_alpha,
            GpuBackend::WebGl2,
            label,
        )
    }
}

/// `request_device` with the adapter's own limits (always grantable; on
/// WebGL2 these are the real WebGL limits) and f16 when offered.
async fn request_device(
    adapter: &wgpu::Adapter,
    label: &'static str,
) -> Result<(wgpu::Device, wgpu::Queue), wgpu::RequestDeviceError> {
    adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some(label),
            required_features: wgpu::Features::SHADER_F16 & adapter.features(),
            required_limits: adapter.limits(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        })
        .await
}

/// A ready GPU for one web `graphics` canvas: device + queue, the configured
/// surface, and the frame target a renderer draws into.
pub struct GpuSurface {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// The adapter the device came from (capability queries).
    adapter: wgpu::Adapter,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    present: PresentBlit,
    /// The graphics primitive keeps this canvas's backing store at the CSS box
    /// × dpr; [`sync_size`](Self::sync_size) re-reads it.
    canvas: HtmlCanvasElement,
    scale: f64,
    backend: GpuBackend,
    label: &'static str,
}

/// A frame acquired by [`GpuSurface::begin_present`].
pub struct PresentFrame(wgpu::SurfaceTexture);

impl GpuSurface {
    #[allow(clippy::too_many_arguments)]
    fn configure(
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: wgpu::Surface<'static>,
        canvas: HtmlCanvasElement,
        formats: &[wgpu::TextureFormat],
        alpha_mode: wgpu::CompositeAlphaMode,
        frame_alpha: FrameAlpha,
        backend: GpuBackend,
        label: &'static str,
    ) -> Option<GpuSurface> {
        let format = choose_surface_format(formats)?;
        let (w, h) = (canvas.width().max(1), canvas.height().max(1));
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: w,
            height: h,
            present_mode: wgpu::PresentMode::AutoVsync,
            // 1, not 2: these are direct-manipulation surfaces (draw / pan /
            // orbit track a finger), where input-to-photon latency matters more
            // than pipelining; a 2-frame queue reads as the view lagging.
            desired_maximum_frame_latency: 1,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        let present =
            PresentBlit::new(&device, format, frame_alpha, surface_wants_premultiplied(alpha_mode), false);
        let (target, target_view) = make_target(&device, w, h, label);
        marker(&format!("{label}: web GPU ({})", backend.name()));
        Some(GpuSurface {
            device,
            queue,
            adapter,
            surface,
            config,
            target,
            target_view,
            present,
            canvas,
            scale: web_dpr(),
            backend,
            label,
        })
    }

    pub fn width(&self) -> u32 {
        self.config.width
    }

    pub fn height(&self) -> u32 {
        self.config.height
    }

    /// Drawable size in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Device pixel ratio (see [`web_dpr`]); refreshed every frame by [`drive`].
    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn label(&self) -> &'static str {
        self.label
    }

    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    /// Always `false` on web (no adopted GL contexts) — present so renderer
    /// code is the same on every target.
    pub fn is_gl(&self) -> bool {
        false
    }

    /// No-op on web (see the native twin) — present so renderer code is the
    /// same on every target.
    pub fn make_current(&self) {}

    pub fn backend(&self) -> GpuBackend {
        self.backend
    }

    pub fn canvas(&self) -> &HtmlCanvasElement {
        &self.canvas
    }

    pub fn target(&self) -> &wgpu::Texture {
        &self.target
    }

    pub fn target_view(&self) -> &wgpu::TextureView {
        &self.target_view
    }

    /// Re-read the dpr and the canvas backing-store size; on a size change,
    /// reconfigure the surface and rebuild the frame target. Returns whether
    /// the size changed (size-dependent renderer resources must then go).
    pub fn sync_size(&mut self) -> bool {
        self.scale = web_dpr();
        let (cw, ch) = (self.canvas.width().max(1), self.canvas.height().max(1));
        if cw == self.config.width && ch == self.config.height {
            return false;
        }
        self.config.width = cw;
        self.config.height = ch;
        self.surface.configure(&self.device, &self.config);
        let (target, target_view) = make_target(&self.device, cw, ch, self.label);
        self.target = target;
        self.target_view = target_view;
        true
    }

    /// Acquire the surface texture and record the frame-target blit into
    /// `encoder`. `None` on timeout/outdated/lost: skip the frame (the next
    /// repaint retries).
    pub fn begin_present(&self, encoder: &mut wgpu::CommandEncoder) -> Option<PresentFrame> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => return None,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.present.draw(&self.device, encoder, &self.target_view, &view);
        Some(PresentFrame(frame))
    }

    /// Present after submitting the encoder that recorded the blit.
    pub fn finish_present(&self, frame: PresentFrame) {
        frame.0.present();
    }
}

/// A renderer's per-surface state (see the native twin): owns the
/// [`GpuSurface`] plus whatever the renderer builds on its device.
pub trait SurfaceState: 'static {
    type Frame: 'static;
    fn gpu(&self) -> &GpuSurface;
    fn gpu_mut(&mut self) -> &mut GpuSurface;
    /// Draw `frame` into the frame target and present it. `true` iff presented.
    fn render(&mut self, frame: &Self::Frame) -> bool;
    /// The canvas was resized ([`GpuSurface::sync_size`] already ran): drop
    /// resources sized to the old drawable.
    fn resized(&mut self) {}
}

/// Per-frame render sink for a view: a GPU [`SurfaceState`] (via [`drive`]) or
/// any other renderer a caller installs on the same canvas (Canvas2D).
pub type RenderFn<F> = Box<dyn FnMut(&F)>;

/// Wrap a [`SurfaceState`] as a [`RenderFn`]: each frame re-syncs the dpr and
/// canvas size (resizing on change), then renders.
pub fn drive<S: SurfaceState>(mut state: S) -> RenderFn<S::Frame> {
    Box::new(move |frame: &S::Frame| {
        if state.gpu_mut().sync_size() {
            state.resized();
        }
        state.render(frame);
    })
}

/// Async renderer bring-up for one `on_ready`.
pub type BuildFuture<F> = Pin<Box<dyn Future<Output = Option<RenderFn<F>>>>>;

/// Create the `graphics` node for a GPU renderer and drive its lifecycle (the
/// web twin of the native `mount`):
///
/// - **paint** runs in a reactive effect owned by the current component
///   scope; each change schedules ONE rAF-aligned render of the latest paint.
/// - **report** is told the LOGICAL size (physical / dpr) on ready and resize.
/// - **build** is spawned on every `on_ready` (after an `on_lost` too) and
///   resolves to the view's [`RenderFn`]; `None` leaves the view blank.
pub fn mount<H, F, P, R, B>(backend: &mut H, paint: P, report: R, build: B) -> H::Node
where
    H: GraphicsOps,
    F: 'static,
    P: Fn() -> F + 'static,
    R: Fn(f32, f32) + Clone + 'static,
    B: Fn(OnReadyEvent) -> BuildFuture<F> + 'static,
{
    let report_logical = move |size: (u32, u32)| {
        let dpr = web_dpr() as f32;
        report(size.0 as f32 / dpr, size.1 as f32 / dpr);
    };
    let frame_cell: Rc<RefCell<Option<F>>> = Rc::new(RefCell::new(None));
    let render_fn: Rc<RefCell<Option<RenderFn<F>>>> = Rc::new(RefCell::new(None));
    // Whether a requestAnimationFrame render is already queued.
    let frame_pending: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    {
        let frame_cell = frame_cell.clone();
        let render_fn = render_fn.clone();
        let frame_pending = frame_pending.clone();
        runtime_world::effect(move || {
            *frame_cell.borrow_mut() = Some(paint());
            schedule_repaint(&render_fn, &frame_cell, &frame_pending);
        });
    }

    let on_ready = {
        let frame_cell = frame_cell.clone();
        let render_fn = render_fn.clone();
        let report_logical = report_logical.clone();
        move |ev: OnReadyEvent| {
            report_logical(ev.size);
            // Blocking is illegal on the wasm main thread: acquire async. Each
            // on_ready (one can follow an on_lost) builds and installs afresh.
            let frame_cell = frame_cell.clone();
            let render_fn = render_fn.clone();
            let fut = build(ev);
            wasm_bindgen_futures::spawn_local(async move {
                if let Some(f) = fut.await {
                    *render_fn.borrow_mut() = Some(f);
                    repaint(&render_fn, &frame_cell);
                }
            });
        }
    };

    let on_resize = {
        let frame_cell = frame_cell.clone();
        let render_fn = render_fn.clone();
        // The renderer re-reads the (already resized) backing store each frame,
        // so a resize only needs a repaint.
        move |ev: OnResizeEvent| {
            report_logical(ev.size);
            repaint(&render_fn, &frame_cell)
        }
    };

    let on_lost = {
        let render_fn = render_fn.clone();
        move || *render_fn.borrow_mut() = None
    };

    backend.create_graphics(
        Box::new(on_ready),
        Box::new(on_resize),
        Box::new(on_lost),
        &AccessibilityProps::default(),
    )
}

/// Queue a render for the next animation frame, coalescing: a burst of
/// reactive updates within one frame collapses to a single render of the
/// LATEST paint.
fn schedule_repaint<F: 'static>(
    render_fn: &Rc<RefCell<Option<RenderFn<F>>>>,
    frame_cell: &Rc<RefCell<Option<F>>>,
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
    let frame_cell = frame_cell.clone();
    let pending_cb = frame_pending.clone();
    // `once_into_js` keeps the closure alive until JS invokes it once.
    let cb = Closure::once_into_js(move || {
        pending_cb.set(false);
        repaint(&render_fn, &frame_cell);
    });
    if window.request_animation_frame(cb.unchecked_ref()).is_err() {
        frame_pending.set(false);
    }
}

/// Run the installed renderer (if any) against the latest paint. Takes the
/// closure out of its cell across the call so a re-entrant signal write in the
/// render path can't double-borrow.
fn repaint<F>(render_fn: &Rc<RefCell<Option<RenderFn<F>>>>, frame_cell: &Rc<RefCell<Option<F>>>) {
    let mut taken = render_fn.borrow_mut().take();
    if let (Some(f), Some(frame)) = (taken.as_mut(), frame_cell.borrow().as_ref()) {
        f(frame);
    }
    // Put it back unless `on_lost` cleared the slot while we rendered.
    let mut slot = render_fn.borrow_mut();
    if slot.is_none() {
        *slot = taken;
    }
}

#[cfg(test)]
mod tests {
    use super::js_truthy_prop;
    use wasm_bindgen::JsValue;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    // `js_sys::Object` / `Reflect` need a JS host — run in the browser.
    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn truthy_prop_gates_on_presence() {
        let obj = js_sys::Object::new();
        assert!(!js_truthy_prop(obj.as_ref(), "gpu"));
        js_sys::Reflect::set(&obj, &JsValue::from_str("gpu"), js_sys::Object::new().as_ref())
            .unwrap();
        assert!(js_truthy_prop(obj.as_ref(), "gpu"));
        js_sys::Reflect::set(&obj, &JsValue::from_str("flag"), &JsValue::FALSE).unwrap();
        assert!(!js_truthy_prop(obj.as_ref(), "flag"));
    }
}
