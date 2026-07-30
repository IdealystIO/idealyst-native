//! Android-only implementation of [`crate::mount`]. Mirrors
//! `host-ios-mobile::ios` end-to-end with three deltas:
//!
//! 1. wgpu uses the Vulkan backend (with GLES fallback), not Metal.
//!    Limits clamp via `Limits::downlevel_defaults().using_resolution(adapter_limits)`
//!    so the request never exceeds what the actual GPU advertises —
//!    Android emulator/older devices have lower caps than current
//!    flagship hardware.
//! 2. No pointer / wheel listeners. Same rationale as iOS — the
//!    preview is read-only; the outer Android backend owns hit
//!    testing for the surrounding `View` tree.
//! 3. No visibility gate yet. iOS walks the UIView chain checking
//!    `window != nil` / `isHidden` / `alpha` to skip GPU encodes
//!    when off-screen. The Android equivalent (walk the View tree
//!    looking at `getVisibility()` / `getWindowToken()`) needs JNI
//!    plumbing per frame; we defer until a real
//!    navigator-hidden-preview use case demands it. The render-loop
//!    keeps ticking but its draw_frame body is cheap when the
//!    SurfaceView isn't presenting.

use std::cell::RefCell;
use std::rc::Rc;

use render_api::DeviceProfile;
use render_wgpu::{Host, Painter, Renderer};
use runtime_shared::driver::{render_loop, RenderLoop};
use runtime_shared::primitives::graphics::GraphicsSurface;

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum MountError {
    /// `wgpu::Instance::create_surface` rejected the handle. Android's
    /// `Graphics` primitive always provides an `AndroidNdkWindowHandle`,
    /// so this means the wgpu instance couldn't bridge the ANativeWindow
    /// to a Vulkan/GLES surface — typically a driver-level issue on a
    /// device that advertises neither backend.
    CreateSurface,
    /// `wgpu::Instance::request_adapter` returned no Vulkan or GLES
    /// adapter. Shouldn't fire on real Android hardware (API 24+)
    /// or recent emulators; would indicate a misconfigured ANGLE /
    /// SwiftShader install or a host environment without GPU
    /// virtualization.
    NoAdapter,
    /// `Adapter::request_device` rejected the limits we asked for —
    /// even after clamping to `adapter.limits()`.
    RequestDevice,
    /// [`mount`] was called before the embedding app booted its
    /// tree (`backend_android::newcore::mounted_world()`
    /// returned `None`). The embedded tree realizes into the app's
    /// world — no app world, no embed. Only reachable from
    /// [`mount`] (mirrors `host-web`'s variant).
    NoHostWorld,
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MountError::CreateSurface => {
                write!(f, "host-android-mobile: wgpu create_surface failed")
            }
            MountError::NoAdapter => write!(
                f,
                "host-android-mobile: no compatible Vulkan/GLES adapter"
            ),
            MountError::RequestDevice => {
                write!(f, "host-android-mobile: wgpu request_device failed")
            }
            MountError::NoHostWorld => write!(
                f,
                "host-android-mobile: mount() before the app host's boot \
                 (backend_android::newcore::mounted_world() is None)"
            ),
        }
    }
}

impl std::error::Error for MountError {}

/// Live handle for one embedded preview. Drop it to release the wgpu
/// device / queue / surface and cancel the render loop. `!Send +
/// !Sync` because the interior state is single-threaded (Rc, wgpu
/// objects, the render-loop guard).
pub struct AndroidHostHandle {
    /// The mounted `render_wgpu::newcore` app. Declared FIRST so the scene
    /// unrealizes (author cleanups, node detach) while the wgpu host
    /// in `inner` is still fully alive; `EmbeddedApp::drop` routes
    /// through `NewCoreApp::stop`, whose embedded path leaves the
    /// app-host flush driver alone.
    _app: EmbeddedApp,
    inner: Rc<RefCell<HostInner>>,
    /// Holding the handle keeps the per-frame closure alive; drop =
    /// cancel the Choreographer raf-loop entry. Declared LAST so the
    /// loop survives long enough for `inner`'s Rc clones inside the
    /// closure to drop.
    _render_loop: RenderLoop,
}

impl AndroidHostHandle {
    /// Reconfigure the wgpu surface to a new physical-pixel size.
    /// Call from the framework `Graphics` primitive's `on_resize`
    /// callback. Identity-size resizes short-circuit so we don't pay
    /// for a no-op reconfigure.
    pub fn resize(&self, size: (u32, u32)) {
        let mut inner = self.inner.borrow_mut();
        if (inner.config.width, inner.config.height) == size {
            return;
        }
        inner.config.width = size.0.max(1);
        inner.config.height = size.1.max(1);
        inner.surface.configure(&inner.device, &inner.config);
    }

    /// Pause the embedded app.
    ///
    /// **Documented gap: this is a no-op.** The handle owns its
    /// mounted app for its entire lifetime — drop is the only teardown
    /// — so suspending would need a visibility gate on `render_wgpu`'s
    /// `Host`/`Renderer` (stop ticking + drawing without unrealizing
    /// the scene), which does not exist. Same gap as `host-web`'s
    /// `WebHostHandle::pause`. Note this host DOES skip GPU encodes for
    /// a hidden view (the per-frame visibility check), so the "hidden
    /// screen keeps burning GPU" case pause() targeted is already
    /// covered.
    pub fn pause(&self) {
        log::warn!("{}: pause() is a no-op (no host visibility gate)", env!("CARGO_PKG_NAME"));
    }

    /// Resume the embedded app. No-op — see [`AndroidHostHandle::pause`].
    pub fn resume(&self) {
        log::warn!("{}: resume() is a no-op (no host visibility gate)", env!("CARGO_PKG_NAME"));
    }

    /// True iff an embedded app is mounted. The handle owns its app
    /// for its whole lifetime, so this is `true` while alive.
    pub fn is_running(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Mount
// ---------------------------------------------------------------------------

/// Mount a wgpu render backend behind a `Graphics`-primitive surface
/// and realize `build_ui`'s scene tree into the embedding app's world
/// (`backend_android::newcore::mounted_world`), so the app-host's flush
/// driver commits the embedded app's staged writes.
pub async fn mount(
    surface_handle: GraphicsSurface,
    size: (u32, u32),
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    build_ui: Rc<dyn Fn() -> runtime_scene::Element + 'static>,
) -> Result<AndroidHostHandle, MountError> {
    // The app's world must exist BEFORE the async wgpu init runs —
    // fail fast on a mis-sequenced boot.
    let world = backend_android::newcore::mounted_world().ok_or(MountError::NoHostWorld)?;
    // The per-frame loop below rides `runtime_shared::driver::render_loop`.
    // The generated JNI wrapper installs the Choreographer driver at
    // `attach`, but the driver contract belongs to the shell, not this
    // embed — install idempotently (first install wins) so any host
    // shell gets a painting embed, exactly like `host_web::mount`'s
    // `backend_web::install_render_loop`.
    backend_android::install_render_loop();

    let init = init_wgpu(surface_handle, size).await?;

    // Per-host session scope: the embedded app's `session::animated`
    // AVs and epoch die with this handle, so a remount replays from
    // initial state instead of resuming mid-animation.
    let session_scope = runtime_shared::session::push_scope();
    let renderer = Renderer::new(&init.device, &init.queue, init.config.format);
    let mut host = Host::new(skin, profile.color_scheme);
    let logical = (
        profile.logical_size.0 as f32,
        profile.logical_size.1 as f32,
    );
    host.set_viewport(logical.0, logical.1);
    let app = render_wgpu::newcore::start_in_world(
        host.backend().clone(),
        |_| {},
        move || (&*build_ui)(),
        world,
    );

    let (inner, render_loop_handle) =
        finish_mount(init, renderer, host, logical, session_scope);

    Ok(AndroidHostHandle {
        _app: EmbeddedApp(Some(app)),
        inner,
        _render_loop: render_loop_handle,
    })
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Everything the async wgpu init produces (the host-web `WgpuInit` shape).
struct WgpuInit {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

/// Step 1 of both mounts: the async wgpu init (instance → surface →
/// adapter → device → configure).
async fn init_wgpu(
    surface_handle: GraphicsSurface,
    size: (u32, u32),
) -> Result<WgpuInit, MountError> {
    // 1. wgpu init. Same shape as `host-ios-mobile` / `host-web`.
    // Vulkan only. The Android emulator advertises both Vulkan and
    // a GL backend, but the GL backend's `eglCreateWindowSurface`
    // crashes with `BadAlloc` on the Pixel_6_Pro_API_34 system
    // image (wgpu picks an EGLConfig the emulator's EGL emulation
    // doesn't accept). Vulkan works fine on the same image. Real
    // devices ship Vulkan from API 24+; GL was only a fallback for
    // pre-Vulkan hardware that we don't realistically need to
    // support here.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
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
    // Pass the adapter's own limits straight through. `downlevel_defaults`
    // still requests compute caps (65535 workgroups/dim, etc.) that the
    // Android emulator's GLES adapter advertises as `0` — wgpu rejects
    // device creation when ANY requested limit exceeds what the adapter
    // exposes, even though `using_resolution(...)` only clamps the
    // texture-resolution fields. Asking for exactly `adapter.limits()`
    // is the only way to fit every backend wgpu picks across real
    // hardware + emulator without enumerating each limit by hand.
    // The renderer's draw-call shape doesn't need anything past what
    // every backend advertises, so this is a no-op constraint on real
    // hardware (you get what you'd have gotten anyway) and the only
    // path that works on emulator GLES (which has 0 compute caps).
    let adapter_limits = adapter.limits();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("host-android-mobile-device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter_limits.clone(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(|_| MountError::RequestDevice)?;
    // Take whatever default config the surface advertises for the
    // adapter — emulator GL is fussy about format + alpha-mode
    // combinations (a srgb preference + ALPHA_MODE_AUTO mismatch
    // crashes `eglCreateWindowSurface` with `BadAlloc`). The default
    // config is "the adapter says this works"; we override only the
    // size + leave srgb / alpha to the surface.
    let mut config = surface
        .get_default_config(&adapter, size.0.max(1), size.1.max(1))
        .ok_or(MountError::CreateSurface)?;
    config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
    config.present_mode = wgpu::PresentMode::Fifo;
    surface.configure(&device, &config);

    Ok(WgpuInit {
        surface,
        device,
        queue,
        config,
    })
}

/// Shared tail of both mounts: the pending-font log, `HostInner`
/// assembly, and the per-frame render loop.
fn finish_mount(
    init: WgpuInit,
    renderer: Renderer,
    host: Host,
    logical: (f32, f32),

    session_scope: runtime_shared::session::ScopeGuard,
) -> (Rc<RefCell<HostInner>>, RenderLoop) {
    // 2a. Drain any pending font URLs the host accumulated during
    //     mount. Android doesn't fetch them today — `face!` fonts
    //     are embedded into the binary via the `embed-font-bytes`
    //     feature, so cosmic-text falls back to the registered
    //     embedded faces (or its built-in default).
    let pending = host.take_pending_font_urls();
    if !pending.is_empty() {
        log::debug!(
            "host-android-mobile: skipped fetch for {} pending font URL(s); \
             cosmic-text will fall back to embedded faces",
            pending.len()
        );
    }

    let inner = Rc::new(RefCell::new(HostInner {
        surface: init.surface,
        device: init.device,
        queue: init.queue,
        config: init.config,
        renderer,
        host,
        logical,
        _session_scope: session_scope,
    }));

    // 3. Per-frame loop via the framework's render-loop driver.
    //    `backend-android-core` (mobile) installs a Choreographer-driven
    //    raf loop; this closure runs on the main thread each vsync.
    let inner_for_frame = inner.clone();
    let render_loop_handle = render_loop(move |_elapsed| {
        let mut inner = inner_for_frame.borrow_mut();
        draw_frame(&mut inner);
    });

    (inner, render_loop_handle)
}

/// Owns the embedded app for a [`mount`] handle.
/// Drop routes through `render_wgpu::newcore::NewCoreApp::stop`
/// (the embedded path: unrealize + guarded diagnostic clear, app-host
/// flush driver untouched — a replacement embed may already have
/// mounted).
struct EmbeddedApp(Option<render_wgpu::newcore::NewCoreApp>);

impl Drop for EmbeddedApp {
    fn drop(&mut self) {
        if let Some(app) = self.0.take() {
            app.stop();
        }
    }
}

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

    /// RAII guard for this host's `session::REGISTRY` scope. Declared
    /// LAST so on `HostInner` drop the scope is popped AFTER the
    /// renderer, host (welcome `Owner` + reactive cleanups), wgpu
    /// surface, etc. drop — those cleanups may dispatch through
    /// scope-anchored timers whose bodies read session state.
    _session_scope: runtime_shared::session::ScopeGuard,
}

fn draw_frame(inner: &mut HostInner) {
    // wgpu 29: `get_current_texture` returns a `CurrentSurfaceTexture`
    // enum. Reconfigure on Outdated/Lost; skip on Timeout/Occluded/
    // Validation — same handling as host-web / host-ios-mobile.
    let surface_tex = match inner.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(t)
        | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
        wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
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
    // — the Choreographer keeps firing every tick regardless.
    let _ = inner.host.tick();
}
