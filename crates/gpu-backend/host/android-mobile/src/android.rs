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
use runtime_core::driver::{render_loop, RenderLoop};
use runtime_core::primitives::graphics::GraphicsSurface;
use runtime_core::Element;

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
        }
    }
}

impl std::error::Error for MountError {}

/// Live handle for one embedded preview. Drop it to release the wgpu
/// device / queue / surface and cancel the render loop. `!Send +
/// !Sync` because the interior state is single-threaded (Rc, wgpu
/// objects, the render-loop guard).
pub struct AndroidHostHandle {
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

    /// Pause the embedded app: drop its reactive scope. Pair with
    /// [`resume`] for navigator-style `unmountOnBlur` semantics — the
    /// next `resume` rebuilds the embedded tree from initial state.
    /// The wgpu device, surface, and renderer stay alive — only the
    /// embedded `build_ui` tree drops — so a subsequent `resume()`
    /// re-mounts fresh without paying the wgpu init cost.
    pub fn pause(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.host.unmount();
    }

    /// Re-mount the embedded app inside the existing wgpu host.
    /// Pairs with [`pause`].
    pub fn resume(&self) {
        let mut inner = self.inner.borrow_mut();
        let build_ui = inner.build_ui.clone();
        inner.host.mount(move || (&*build_ui)());
    }

    /// Whether the embedded app is currently mounted (false after
    /// [`pause`], true after [`resume`] / initial mount). Used by
    /// SDK-level visibility plumbing.
    pub fn is_running(&self) -> bool {
        self.inner.borrow().host.is_mounted()
    }
}

// ---------------------------------------------------------------------------
// Mount
// ---------------------------------------------------------------------------

/// Mount a wgpu render backend behind an Android `Graphics` primitive
/// surface. Returns an `AndroidHostHandle` whose drop tears everything
/// down. Call from the `Graphics` primitive's `on_ready` callback;
/// stash the handle so `on_resize` can call [`AndroidHostHandle::resize`]
/// and `on_lost` can drop it.
pub async fn mount(
    surface_handle: GraphicsSurface,
    size: (u32, u32),
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    build_ui: Rc<dyn Fn() -> Element + 'static>,
) -> Result<AndroidHostHandle, MountError> {
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

    // 2. Build the render-side stack + mount the user app.
    //
    // Push a fresh `session::REGISTRY` scope so the embedded app's
    // `session::animated(…)` / `session::epoch_micros()` calls land in
    // a per-host registry that disappears when this host's
    // `AndroidHostHandle` drops. Matches the iOS host behavior —
    // navigators using `MountPolicy::LazyDisposing` can truly reset
    // the embedded app on remount.
    let session_scope = runtime_core::session::push_scope();
    let renderer = Renderer::new(&device, &queue, config.format);
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

    // 2a. Drain any pending font URLs the host accumulated during
    //     `mount`. Android doesn't fetch them today — `face!` fonts
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
        surface,
        device,
        queue,
        config,
        renderer,
        host,
        logical,
        build_ui,
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

    Ok(AndroidHostHandle {
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
    /// Re-callable embedded-app builder. Cached so a paused→resumed
    /// transition can remount the welcome subtree without bouncing
    /// the wgpu device/surface.
    build_ui: Rc<dyn Fn() -> Element + 'static>,
    /// RAII guard for this host's `session::REGISTRY` scope. Declared
    /// LAST so on `HostInner` drop the scope is popped AFTER the
    /// renderer, host (welcome `Owner` + reactive cleanups), wgpu
    /// surface, etc. drop — those cleanups may dispatch through
    /// scope-anchored timers whose bodies read session state.
    _session_scope: runtime_core::session::ScopeGuard,
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
