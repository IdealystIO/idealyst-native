//! iOS-only implementation of [`crate::mount`]. Mirrors
//! `host-web::web` end-to-end with three deltas:
//!
//! 1. wgpu uses the Metal backend, not WebGL2. Limits stay at the
//!    adapter's defaults — modern Apple silicon ships every limit we
//!    care about higher than `downlevel_webgl2_defaults`.
//! 2. No pointer / wheel listeners. The preview is read-only on iOS
//!    (the outer iOS backend owns hit testing for the surrounding
//!    `UIView` tree; touches on the preview surface don't have a
//!    meaningful target inside the wgpu scene).
//! 3. No font fetching. `face!` fonts on iOS are embedded into the
//!    binary by the `embed-font-bytes` feature, so the wgpu Host's
//!    cosmic-text shaper falls back to its built-in default face
//!    when the registered fonts aren't bytes-backed. The result is
//!    visually close enough for a preview embed; if pixel-perfect
//!    typography matters in a future use case, plumb the bytes from
//!    `runtime_core::assets::*` here.

use std::cell::RefCell;
use std::rc::Rc;

use raw_window_handle::HasWindowHandle;
use render_api::DeviceProfile;
use render_wgpu::{Host, Painter, Renderer};
use runtime_core::driver::{render_loop, RenderLoop};
use runtime_core::primitives::graphics::GraphicsSurface;
use runtime_core::Element;

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum MountError {
    /// `GraphicsSurface` doesn't expose a UiKit window handle. The iOS
    /// `Graphics` primitive always provides one, so this should only
    /// fire on a misuse (e.g. handing in a web `CanvasSurfaceProvider`).
    NoUiKitHandle,
    /// `wgpu::Instance::create_surface` rejected the handle.
    CreateSurface,
    /// `wgpu::Instance::request_adapter` returned no Metal adapter.
    /// Shouldn't happen on real iOS hardware; would only fire if
    /// Metal is somehow disabled (broken simulator install,
    /// out-of-process render constraints).
    NoAdapter,
    /// `Adapter::request_device` rejected the limits we asked for.
    RequestDevice,
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MountError::NoUiKitHandle => {
                write!(f, "host-ios-mobile: GraphicsSurface has no UiKit window handle")
            }
            MountError::CreateSurface => {
                write!(f, "host-ios-mobile: wgpu create_surface failed")
            }
            MountError::NoAdapter => write!(f, "host-ios-mobile: no compatible Metal adapter"),
            MountError::RequestDevice => write!(f, "host-ios-mobile: wgpu request_device failed"),
        }
    }
}

impl std::error::Error for MountError {}

/// Live handle for one embedded preview. Drop it to release the wgpu
/// device / queue / surface and cancel the render loop. `!Send +
/// !Sync` because the interior state is single-threaded (Rc, wgpu
/// objects, the render-loop guard).
pub struct IosHostHandle {
    inner: Rc<RefCell<HostInner>>,
    /// Holding the handle keeps the per-frame closure alive; drop =
    /// cancel the NSTimer. Declared LAST so the loop survives long
    /// enough for `inner`'s Rc clones inside the closure to drop.
    _render_loop: RenderLoop,
}

impl IosHostHandle {
    /// Reconfigure the wgpu surface to a new physical-pixel size.
    /// Call from the framework `Graphics` primitive's `on_resize`
    /// callback. Identity-size resizes (same dims as the current
    /// config) short-circuit so we don't pay for a no-op reconfigure.
    pub fn resize(&self, size: (u32, u32)) {
        let mut inner = self.inner.borrow_mut();
        if (inner.config.width, inner.config.height) == size {
            return;
        }
        inner.config.width = size.0.max(1);
        inner.config.height = size.1.max(1);
        inner.surface.configure(&inner.device, &inner.config);
    }
}

/// Mount the wgpu render backend behind a framework `Graphics`
/// surface on iOS. Call from inside the surface's `on_ready`, stash
/// the returned handle so `on_resize` / `on_lost` can reconfigure or
/// drop it.
pub async fn mount<F>(
    surface_handle: GraphicsSurface,
    size: (u32, u32),
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    build_ui: F,
) -> Result<IosHostHandle, MountError>
where
    F: FnOnce() -> Element + 'static,
{
    // 1. Validate the surface exposes a UiKit handle. The iOS
    //    `Graphics` primitive always provides one (see
    //    `backend-ios-mobile/src/imp/graphics.rs::IosSurfaceProvider`);
    //    a missing handle means the caller wired a non-iOS surface in.
    surface_handle
        .window_handle()
        .map_err(|_| MountError::NoUiKitHandle)?;

    // 2. wgpu init. `wgpu 29` requires explicit `InstanceDescriptor`
    //    fields — see `host-web` for the matching set on the GL path.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        flags: wgpu::InstanceFlags::empty(),
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions::default(),
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
    // Constrain limits to what the actual Metal adapter exposes.
    // The iOS Simulator's Metal implementation reports lower
    // caps than real hardware (no `non_uniform_indexing`, lower
    // `max_storage_buffers_per_shader_stage`, etc.); requesting
    // `wgpu::Limits::default()` fails with `RequestDeviceError` at
    // device creation. `adapter.limits()` is the safe pick — it's
    // exactly what the adapter advertises, so the
    // `using_resolution(...)` clamp ensures we never ask for more
    // than what's available on either simulator or device.
    let adapter_limits = adapter.limits();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("host-ios-mobile-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults()
                .using_resolution(adapter_limits),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(|_| MountError::RequestDevice)?;
    let caps = surface.get_capabilities(&adapter);
    // sRGB-encoded so CSS-style hex values render without manual
    // gamma encoding (same pick as host-web).
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(caps.formats[0]);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.0.max(1),
        height: size.1.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
    };
    surface.configure(&device, &config);

    // 3. Build the render-side stack + mount the user app.
    let renderer = Renderer::new(&device, &queue, config.format);
    let mut host = Host::new(skin, profile.color_scheme);
    let logical = (
        profile.logical_size.0 as f32,
        profile.logical_size.1 as f32,
    );
    host.set_viewport(logical.0, logical.1);
    host.mount(build_ui);

    // 3a. Drain any pending font URLs the host accumulated during
    //     `mount`. iOS doesn't fetch them; logging the count helps
    //     diagnose missing-font issues if a future change starts
    //     populating this queue.
    let pending = host.take_pending_font_urls();
    if !pending.is_empty() {
        log::debug!(
            "host-ios-mobile: skipped fetch for {} pending font URL(s); \
             cosmic-text will fall back to embedded faces",
            pending.len()
        );
    }

    let inner = Rc::new(RefCell::new(HostInner {
        surface,
        device,
        queue,
        config,
        renderer,
        host,
        logical,
    }));

    // 4. Per-frame loop via the framework's render-loop driver.
    //    `backend-ios-core` installs an NSTimer at ~60 Hz; this
    //    closure runs on the main thread each tick.
    let inner_for_frame = inner.clone();
    let render_loop_handle = render_loop(move |_elapsed| {
        let mut inner = inner_for_frame.borrow_mut();
        draw_frame(&mut inner);
    });

    Ok(IosHostHandle {
        inner,
        _render_loop: render_loop_handle,
    })
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

struct HostInner {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    host: Host,
    /// Logical viewport in CSS px from the `DeviceProfile`. Fed to
    /// the renderer every frame.
    logical: (f32, f32),
}

fn draw_frame(inner: &mut HostInner) {
    // wgpu 29: `get_current_texture` returns a `CurrentSurfaceTexture`
    // enum. Reconfigure on Outdated/Lost; skip on Timeout/Occluded/
    // Validation — same handling as host-web.
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
    // Advance per-frame state (animations, spinners, momentum). The
    // return value (true while anims are in flight) doesn't matter
    // — NSTimer keeps firing every tick regardless. The host's
    // redraw hook covers signal flips that happen between frames.
    let _ = inner.host.tick();
}
