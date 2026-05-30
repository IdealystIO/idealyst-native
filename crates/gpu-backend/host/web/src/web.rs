//! Wasm32 implementation of `host-web` — see the crate root for the
//! big-picture story. This module is the actual code; everything in
//! `lib.rs` is just re-exports + the non-wasm stub.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::driver::{render_loop, RenderLoop};
use runtime_core::primitives::graphics::GraphicsSurface;
use runtime_core::Element;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use render_api::{DeviceProfile, PointerButton, PointerEvent, PointerId, ScrollEvent};
use render_wgpu::{Host, Renderer, Painter};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum MountError {
    /// The `GraphicsSurface` didn't expose a `WebCanvasWindowHandle`
    /// — happens if a caller wires a non-web backend's surface
    /// through. The web `Graphics` primitive always uses
    /// `CanvasSurfaceProvider`, so this should only fire on a misuse.
    NoCanvas,
    /// Couldn't open a wgpu surface against the canvas.
    CreateSurface,
    /// No GPU adapter satisfies the WebGL2 limits the host needs.
    NoAdapter,
    /// `request_device` rejected — usually the limits don't match
    /// what the browser exposes.
    RequestDevice,
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MountError::NoCanvas => write!(f, "host-web: GraphicsSurface has no WebCanvas window handle"),
            MountError::CreateSurface => write!(f, "host-web: wgpu create_surface failed"),
            MountError::NoAdapter => write!(f, "host-web: no compatible WebGL2 adapter"),
            MountError::RequestDevice => write!(f, "host-web: wgpu request_device failed"),
        }
    }
}

impl std::error::Error for MountError {}

/// Live handle for one embedded preview. Drop it to tear everything
/// down (removes the JS listeners, cancels the render loop, releases
/// the wgpu objects, clears the host's reactive scopes).
///
/// `!Send + !Sync` because every interior piece — wgpu handles, the
/// JS closures, the `Rc` — is single-threaded.
pub struct WebHostHandle {
    inner: Rc<RefCell<HostInner>>,
    /// Held to keep the JS listeners alive and so `Drop` removes
    /// them. Declared BEFORE `_render_loop` so the loop survives
    /// past the listener drop — a queued pointer event firing
    /// during teardown won't reach a dropped host because the
    /// listener already unhooked itself.
    _listeners: Vec<EventListener>,
    /// Holding the handle keeps the per-frame closure alive; drop =
    /// cancel rAF. Declared LAST so the loop survives long enough
    /// for `inner` and `_listeners` to drop their `Rc` clones.
    _render_loop: RenderLoop,
}

impl WebHostHandle {
    /// Reconfigure the wgpu surface to a new physical-pixel size.
    /// Call from the `Graphics` primitive's `on_resize` callback.
    /// Sizes are clamped to [`MAX_SURFACE_EXTENT_PX`] so a high-DPR
    /// canvas can't blow through the WebGL2 `MAX_TEXTURE_SIZE` cap.
    /// Identity-size resizes (same dims as the current config)
    /// short-circuit so we don't pay for a no-op reconfigure.
    pub fn resize(&self, size: (u32, u32)) {
        let clamped = clamp_surface(size);
        let mut inner = self.inner.borrow_mut();
        if (inner.config.width, inner.config.height) == clamped {
            return;
        }
        inner.config.width = clamped.0;
        inner.config.height = clamped.1;
        inner.surface.configure(&inner.device, &inner.config);
    }

    /// Pause the embedded app: drop its reactive scope so all of its
    /// effects, `AnimatedValue` subscribers, and per-frame work
    /// stop firing. Pair with [`resume`].
    ///
    /// Web today doesn't auto-detect visibility (a future
    /// `IntersectionObserver`-driven hook can flip this on its own),
    /// so callers must wire it themselves — typically inside a
    /// reactive effect bound to `use_focus()`.
    pub fn pause(&self) {
        self.inner.borrow_mut().host.unmount();
        runtime_core::session::clear();
    }

    /// Re-mount the embedded app from its cached `build_ui`.
    /// Idempotent. Pair with [`pause`].
    pub fn resume(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.host.is_mounted() {
            return;
        }
        let build_ui = inner.build_ui.clone();
        inner.host.mount(move || (&*build_ui)());
    }

    pub fn is_running(&self) -> bool {
        self.inner.borrow().host.is_mounted()
    }
}

/// Mount the wgpu render backend behind a framework `Graphics`
/// surface. This is the only entry point — call from the surface's
/// `on_ready`, hand the returned handle to the surrounding state so
/// `on_resize` / `on_lost` can `.resize(...)` / drop it.
pub async fn mount(
    surface_handle: GraphicsSurface,
    size: (u32, u32),
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    // `Rc<dyn Fn>` matches the iOS host's signature (which uses it
    // for unmount/remount on visibility-gated frame skips). Web
    // doesn't unmount yet — calls it once below — but the umbrella
    // crate's signature is shared.
    build_ui: Rc<dyn Fn() -> Element + 'static>,
) -> Result<WebHostHandle, MountError> {
    // 1. Extract the canvas. Keep a clone — the surface gets
    //    consumed by `create_surface` below; we need the canvas
    //    later to attach event listeners and read its bounding
    //    rect on every pointer event.
    let canvas = extract_canvas(&surface_handle).ok_or(MountError::NoCanvas)?;

    // 2. wgpu init. WebGL2-only; see the crate doc for why.
    //
    // wgpu 29: `InstanceDescriptor` no longer implements `Default`
    // and gained `memory_budget_thresholds` / `backend_options` /
    // `display`; pass explicit defaults. `request_adapter` now
    // returns `Result`, and `request_device` takes one arg
    // (descriptor) and gained `experimental_features` + `trace`.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::GL,
        // Empty validation flags — Web's WebGPU validation paths
        // aren't universally supported. Same conservative pick as
        // the gradient demo.
        flags: wgpu::InstanceFlags::empty(),
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions::default(),
        // GLES/Wayland need the explicit display handle on native;
        // on the web the per-surface canvas handle is sufficient.
        display: None,
    });
    let surface = instance
        .create_surface(surface_handle)
        .map_err(|_| MountError::CreateSurface)?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
        .map_err(|_| MountError::NoAdapter)?;
    // Static WebGL2 defaults — DON'T call
    // `.using_resolution(adapter.limits())` because that read
    // touches `maxInterStageShaderComponents`, which modern Chrome
    // panics on (see crate doc).
    let limits = wgpu::Limits::downlevel_webgl2_defaults();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("host-web-device"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(|_| MountError::RequestDevice)?;
    let caps = surface.get_capabilities(&adapter);
    // sRGB-encoded format so CSS-style hex values render without
    // manual gamma encoding. Same pick as host-winit's GPU init.
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(caps.formats[0]);
    let clamped = clamp_surface(size);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: clamped.0,
        height: clamped.1,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
    };
    surface.configure(&device, &config);

    // 3. Build the render-side stack + mount the user app.
    let mut renderer = Renderer::new(&device, &queue, config.format);
    let mut host = Host::new(skin, profile.color_scheme);
    let logical = (
        profile.logical_size.0 as f32,
        profile.logical_size.1 as f32,
    );
    host.set_viewport(logical.0, logical.1);
    {
        let build_ui = build_ui.clone();
        host.mount(move || (&*build_ui)());
    }

    // 3a. Fonts. With `embed-font-bytes` off for web, `face!` fonts
    //     aren't baked into the wasm — they're served files at the
    //     same `/fonts/*.ttf` URLs the DOM backend links via
    //     `@font-face`. Mounting registered each font's URL; fetch
    //     them now and feed the wgpu text shaper *before* the first
    //     frame, so text shapes against its real face with no
    //     fallback-font flash. The engine's embedded default
    //     (Inter-Regular, baked unconditionally) covers any fetch
    //     that fails. Awaited here because `mount` is already async;
    //     the per-frame loop (step 4) starts only after fonts land.
    let font_urls = host.take_pending_font_urls();
    let mut loaded_any = false;
    for url in &font_urls {
        match fetch_font_bytes(url).await {
            Some(bytes) => {
                host.load_font_bytes(bytes);
                loaded_any = true;
            }
            None => web_sys::console::warn_1(
                &format!(
                    "host-web: font fetch failed for {url}; \
                     text falls back to the embedded default face"
                )
                .into(),
            ),
        }
    }
    if loaded_any {
        // Text shaped during mount used only the embedded default;
        // re-measure so the now-loaded faces take effect on frame 1.
        host.invalidate_text_layout();
    }

    let inner = Rc::new(RefCell::new(HostInner {
        surface,
        device,
        queue,
        config,
        renderer,
        host,
        logical,
        canvas: canvas.clone(),
        build_ui,
    }));

    // 4. Per-frame loop. The closure borrows the inner mut; pointer
    //    listeners borrow it mut too, but JS dispatches them
    //    sequentially with rAF so they never overlap.
    let inner_for_frame = inner.clone();
    let render_loop_handle = render_loop(move |_elapsed| {
        let mut inner = inner_for_frame.borrow_mut();
        draw_frame(&mut inner);
    });

    // 5. Input plumbing. The listeners' closures each hold their own
    //    `Rc` clone of `inner` so events still flow even if the
    //    caller drops the `inner` field of the handle (it won't,
    //    but the `Rc` keeps the API forgiving).
    let listeners = install_listeners(&canvas, inner.clone());

    Ok(WebHostHandle {
        inner,
        _listeners: listeners,
        _render_loop: render_loop_handle,
    })
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Fetch a served font file and return its raw bytes. Returns `None`
/// on any failure (no window, network error, non-2xx, decode error) —
/// the caller logs and falls back to the embedded default face, so a
/// missing font degrades gracefully rather than panicking the mount.
async fn fetch_font_bytes(url: &str) -> Option<Vec<u8>> {
    let window = web_sys::window()?;
    let resp_value = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .ok()?;
    let resp: web_sys::Response = resp_value.dyn_into().ok()?;
    if !resp.ok() {
        return None;
    }
    let buf = wasm_bindgen_futures::JsFuture::from(resp.array_buffer().ok()?)
        .await
        .ok()?;
    Some(js_sys::Uint8Array::new(&buf).to_vec())
}

/// Maximum physical-pixel extent the surface is allowed to take in
/// either dimension. WebGL2 caps `MAX_TEXTURE_SIZE` at 2048 on a
/// wide range of devices; configuring a wgpu surface larger panics
/// out of `wgpu::backend::wgpu_core::handle_error_fatal`. Clamping
/// here means callers don't have to think about it. WebGPU permits
/// 8192 on modern Chrome, but we're on the GL backend path; staying
/// with the GL cap keeps the bridge robust under any browser.
pub const MAX_SURFACE_EXTENT_PX: u32 = 2048;

fn clamp_surface(size: (u32, u32)) -> (u32, u32) {
    (
        size.0.min(MAX_SURFACE_EXTENT_PX).max(1),
        size.1.min(MAX_SURFACE_EXTENT_PX).max(1),
    )
}

struct HostInner {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    host: Host,
    /// Logical viewport in CSS px. Pinned by the device profile;
    /// fed to the renderer every frame and used to translate
    /// canvas-relative pointer events into host coordinates.
    logical: (f32, f32),
    /// Canvas the listeners are attached to. We read the bounding
    /// rect on every pointer event so the canvas-relative position
    /// stays accurate even as the page reflows (a sidebar opening,
    /// a window resize, etc.).
    canvas: web_sys::HtmlCanvasElement,
    /// Re-callable embedded-app builder. Cached so [`WebHostHandle::resume`]
    /// can re-mount after a [`pause`].
    build_ui: Rc<dyn Fn() -> Element + 'static>,
}

/// One installed JS event listener. Drop = `removeEventListener` +
/// the wasm-bindgen closure releases. Wrapping it so the
/// `WebHostHandle` can hold a uniform `Vec<EventListener>` without
/// caring about per-event types.
struct EventListener {
    target: web_sys::EventTarget,
    event: &'static str,
    closure: Closure<dyn FnMut(JsValue)>,
}

impl Drop for EventListener {
    fn drop(&mut self) {
        // Remove first, then let the closure drop afterward.
        // Reversing this order risks `__wbindgen_destroy_closure`
        // panicking on a callback still queued in the JS event
        // loop. Same pattern as backend-web's scheduler uses for
        // its `OneShotInner` cleanup.
        let _ = self.target.remove_event_listener_with_callback(
            self.event,
            self.closure.as_ref().unchecked_ref(),
        );
    }
}

/// Pull the underlying `HtmlCanvasElement` out of the framework's
/// opaque `GraphicsSurface`. The web backend's
/// `CanvasSurfaceProvider::window_handle()` packs the canvas's
/// `JsValue` pointer into `WebCanvasWindowHandle.obj` (see
/// `backend-web/src/primitives/graphics.rs`); we read it back here.
///
/// The reverse cast is the standard pattern — wgpu's web backend
/// does the same to bind a surface to a canvas.
fn extract_canvas(surface: &GraphicsSurface) -> Option<web_sys::HtmlCanvasElement> {
    let handle = surface.window_handle().ok()?;
    let RawWindowHandle::WebCanvas(h) = handle.as_raw() else { return None };
    // SAFETY: `WebCanvasWindowHandle::new` stored a pointer to the
    // canvas's `JsValue` (in `CanvasSurfaceProvider::window_handle`)
    // whose lifetime is tied to the surface we hold. Treating it as
    // `&JsValue` is sound for the duration of this function; the
    // clone bumps the refcount before the borrow ends.
    let js_val: &JsValue = unsafe { &*(h.obj.as_ptr() as *const JsValue) };
    js_val
        .clone()
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .ok()
}

fn draw_frame(inner: &mut HostInner) {
    // wgpu 29: `get_current_texture` returns a `CurrentSurfaceTexture`
    // *enum* (the pre-29 `Result<SurfaceTexture, SurfaceError>` is
    // gone). Reconfigure on Lost/Outdated; skip the frame on any
    // other non-Success outcome.
    let surface_tex = match inner.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(t)
        | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
        wgpu::CurrentSurfaceTexture::Outdated
        | wgpu::CurrentSurfaceTexture::Lost => {
            inner.surface.configure(&inner.device, &inner.config);
            return;
        }
        wgpu::CurrentSurfaceTexture::Timeout
        | wgpu::CurrentSurfaceTexture::Occluded
        | wgpu::CurrentSurfaceTexture::Validation => return,
    };
    let view = surface_tex
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    inner.renderer.render(
        &inner.host,
        &inner.device,
        &inner.queue,
        &view,
        inner.logical,
        (
            0.0,
            0.0,
            inner.config.width as f32,
            inner.config.height as f32,
        ),
    );
    surface_tex.present();
    // Advance per-frame state (caret blink, spinners, tween engine,
    // momentum). Return value (`true` while anims in flight) doesn't
    // matter here — `rAF` keeps firing every frame regardless. The
    // host's redraw hook covers state changes that happen between
    // frames (signal flips during a pointer handler etc.).
    let _ = inner.host.tick();
}

// ---------------------------------------------------------------------------
// Pointer / wheel translation
// ---------------------------------------------------------------------------

/// Where the canvas sits in the viewport, in CSS px. Read on every
/// pointer event so we don't grow stale when the page reflows
/// (sidebar opens, ancestors resize, etc.).
fn canvas_to_logical(inner: &HostInner, client_x: f64, client_y: f64) -> (f32, f32) {
    let rect = inner.canvas.get_bounding_client_rect();
    let css_w = (rect.width() as f32).max(1.0);
    let css_h = (rect.height() as f32).max(1.0);
    let x = ((client_x - rect.left()) as f32) * inner.logical.0 / css_w;
    let y = ((client_y - rect.top()) as f32) * inner.logical.1 / css_h;
    (x, y)
}

/// DOM `MouseEvent.button` → `render_api::PointerButton`.
/// 0 = primary, 1 = middle, 2 = secondary, 3 = back, 4 = forward.
/// Touch and pen don't set `button` (default 0 = Primary).
fn pointer_button_from_dom(b: i16) -> PointerButton {
    match b {
        0 => PointerButton::Primary,
        1 => PointerButton::Middle,
        2 => PointerButton::Secondary,
        n if n >= 0 => PointerButton::Other(n as u16),
        _ => PointerButton::Primary,
    }
}

#[derive(Clone, Copy)]
enum PointerPhase {
    Down,
    Move,
    Up,
}

fn install_listeners(
    canvas: &web_sys::HtmlCanvasElement,
    inner: Rc<RefCell<HostInner>>,
) -> Vec<EventListener> {
    let target: web_sys::EventTarget = canvas.clone().into();
    vec![
        pointer_listener(target.clone(), "pointerdown", inner.clone(), PointerPhase::Down),
        pointer_listener(target.clone(), "pointermove", inner.clone(), PointerPhase::Move),
        pointer_listener(target.clone(), "pointerup", inner.clone(), PointerPhase::Up),
        pointer_cancel_listener(target.clone(), inner.clone()),
        wheel_listener(target, inner),
    ]
}

fn pointer_listener(
    target: web_sys::EventTarget,
    event: &'static str,
    inner: Rc<RefCell<HostInner>>,
    phase: PointerPhase,
) -> EventListener {
    let closure: Closure<dyn FnMut(JsValue)> = Closure::new(move |jsv: JsValue| {
        let Ok(ev) = jsv.dyn_into::<web_sys::PointerEvent>() else { return };
        // Stop text-selection / native drag from kicking in over
        // the canvas — the embedded app does its own gesture work.
        ev.prevent_default();
        let mut inner = match inner.try_borrow_mut() {
            Ok(g) => g,
            // Mid-frame the render loop holds the borrow. JS is
            // single-threaded so this normally never trips, but if
            // a rAF callback yields into another synchronous JS
            // callback it can. Skipping the event is the right
            // call — a missed pointermove costs nothing.
            Err(_) => return,
        };
        // Capture the pointer on `pointerdown` so subsequent
        // `pointermove` / `pointerup` events keep firing on the
        // canvas even when the pointer leaves it mid-drag. Without
        // this the browser routes off-canvas events to whatever
        // element the pointer is over, which in our case means the
        // host never sees `pointerup` — leaving a button stuck in
        // its press state, a slider locked to its drag, a scroll
        // gesture coasting indefinitely. Capture releases
        // automatically when the pointer is released (per the
        // Pointer Events spec, ImplicitRelease step), so there's
        // nothing to undo on `Up`.
        if matches!(phase, PointerPhase::Down) {
            let _ = inner.canvas.set_pointer_capture(ev.pointer_id());
        }
        let pos = canvas_to_logical(&inner, ev.client_x() as f64, ev.client_y() as f64);
        let pe = PointerEvent {
            id: PointerId(ev.pointer_id() as u64),
            button: pointer_button_from_dom(ev.button()),
            position: pos,
        };
        match phase {
            PointerPhase::Down => inner.host.pointer_down(pe),
            PointerPhase::Move => inner.host.pointer_move(pe),
            PointerPhase::Up => inner.host.pointer_up(pe),
        }
    });
    let _ = target
        .add_event_listener_with_callback(event, closure.as_ref().unchecked_ref());
    EventListener {
        target,
        event,
        closure,
    }
}

fn pointer_cancel_listener(
    target: web_sys::EventTarget,
    inner: Rc<RefCell<HostInner>>,
) -> EventListener {
    let closure: Closure<dyn FnMut(JsValue)> = Closure::new(move |_jsv: JsValue| {
        let Ok(mut inner) = inner.try_borrow_mut() else { return };
        // `pointercancel` has no meaningful position by the time
        // it fires — the spec confirms it, and `EventSink` takes
        // no event for that reason.
        inner.host.pointer_cancel();
    });
    let _ = target.add_event_listener_with_callback(
        "pointercancel",
        closure.as_ref().unchecked_ref(),
    );
    EventListener {
        target,
        event: "pointercancel",
        closure,
    }
}

fn wheel_listener(
    target: web_sys::EventTarget,
    inner: Rc<RefCell<HostInner>>,
) -> EventListener {
    let closure: Closure<dyn FnMut(JsValue)> = Closure::new(move |jsv: JsValue| {
        let Ok(ev) = jsv.dyn_into::<web_sys::WheelEvent>() else { return };
        // Prevent the surrounding page from scrolling when the user
        // wheels over the embedded canvas — the inner app's own
        // scroll views should consume the delta instead.
        ev.prevent_default();
        let Ok(mut inner) = inner.try_borrow_mut() else { return };
        let pos = canvas_to_logical(&inner, ev.client_x() as f64, ev.client_y() as f64);
        // `WheelEvent.delta{X,Y}` are in CSS px when `deltaMode == 0`
        // (the common case). The other modes are lines (1) and
        // pages (2); treating them as px is wrong but harmless —
        // the magnitude differs but the gesture still scrolls in
        // the right direction. Refine if a use case lands.
        //
        // Sign convention: browsers report positive `deltaY` as
        // "wheel down → reveal content below"; render-api's
        // convention is "positive delta.y scrolls content up", so
        // we invert.
        let css_w = (inner.canvas.get_bounding_client_rect().width() as f32).max(1.0);
        let css_h = (inner.canvas.get_bounding_client_rect().height() as f32).max(1.0);
        let scale_x = inner.logical.0 / css_w;
        let scale_y = inner.logical.1 / css_h;
        let delta = (
            -(ev.delta_x() as f32) * scale_x,
            -(ev.delta_y() as f32) * scale_y,
        );
        inner.host.scroll(ScrollEvent { position: pos, delta });
    });
    let _ = target
        .add_event_listener_with_callback("wheel", closure.as_ref().unchecked_ref());
    EventListener {
        target,
        event: "wheel",
        closure,
    }
}
