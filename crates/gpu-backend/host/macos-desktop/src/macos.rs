//! macOS-only implementation of [`crate::mount`]. Mirrors
//! `host-ios-mobile::ios` end-to-end with two deltas:
//!
//! 1. The surface exposes an AppKit window handle (`ns_view`), not a
//!    UiKit one. Same Metal wgpu backend, same adapter-limits clamp
//!    (harmless on desktop — the adapter advertises more than
//!    `downlevel_defaults` asks for).
//! 2. The visibility walk speaks NSView: `alphaValue` instead of
//!    UIView's `alpha`, plus the same `window != nil` and recursive
//!    `isHidden` checks.
//!
//! Font behavior matches iOS: no fetching — `face!` fonts on macOS
//! are embedded into the binary by the `embed-font-bytes` feature, so
//! the wgpu Host's cosmic-text shaper falls back to its built-in
//! default face when the registered fonts aren't bytes-backed.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::msg_send;
use objc2_foundation::NSObject;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
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
    /// `GraphicsSurface` doesn't expose an AppKit window handle. The
    /// macOS `Graphics` primitive always provides one, so this should
    /// only fire on a misuse (e.g. handing in a web
    /// `CanvasSurfaceProvider`).
    NoAppKitHandle,
    /// `wgpu::Instance::create_surface` rejected the handle.
    CreateSurface,
    /// `wgpu::Instance::request_adapter` returned no Metal adapter.
    NoAdapter,
    /// `Adapter::request_device` rejected the limits we asked for.
    RequestDevice,
    /// `mount_newcore` was called before the embedding app booted a
    /// new-core tree (`backend_macos::newcore::mounted_world()`
    /// returned `None`). The embedded tree realizes into the app's
    /// world — no app world, no embed. Only reachable from
    /// `mount_newcore` (mirrors `host-web`'s variant).
    NoHostWorld,
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MountError::NoAppKitHandle => {
                write!(f, "host-macos-desktop: GraphicsSurface has no AppKit window handle")
            }
            MountError::CreateSurface => {
                write!(f, "host-macos-desktop: wgpu create_surface failed")
            }
            MountError::NoAdapter => {
                write!(f, "host-macos-desktop: no compatible Metal adapter")
            }
            MountError::RequestDevice => {
                write!(f, "host-macos-desktop: wgpu request_device failed")
            }
            MountError::NoHostWorld => write!(
                f,
                "host-macos-desktop: mount_newcore before the app's new-core boot \
                 (backend_macos::newcore::mounted_world() is None)"
            ),
        }
    }
}

impl std::error::Error for MountError {}

/// Live handle for one embedded preview. Drop it to release the wgpu
/// device / queue / surface and cancel the render loop. `!Send +
/// !Sync` because the interior state is single-threaded (Rc, wgpu
/// objects, the render-loop guard).
pub struct MacosHostHandle {
    /// New-core embeds only ([`mount_newcore`]): the mounted
    /// `render_wgpu::newcore` app. Declared FIRST so the scene
    /// unrealizes (author cleanups, node detach) while the wgpu host
    /// in `inner` is still fully alive; `NewCoreGuard::drop` routes
    /// through `NewCoreApp::stop`, whose embedded path leaves the
    /// app-host flush driver alone.
    #[cfg(feature = "new-core")]
    _newcore: Option<NewCoreGuard>,
    inner: Rc<RefCell<HostInner>>,
    /// Holding the handle keeps the per-frame closure alive; drop =
    /// cancel the NSTimer. Declared LAST so the loop survives long
    /// enough for `inner`'s Rc clones inside the closure to drop.
    _render_loop: RenderLoop,
}

impl MacosHostHandle {
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
    /// [`resume`] for `unmountOnBlur` semantics. The wgpu device,
    /// surface, and renderer stay alive — only the embedded
    /// `build_ui` tree drops — so a subsequent `resume()` re-mounts
    /// fresh without paying the wgpu init cost. Same caveats as the
    /// iOS host (see `IosHostHandle::pause`).
    ///
    /// Old-core mounts only: a [`mount_newcore`] handle's app is owned
    /// by the handle itself (drop = full teardown), and `Host::unmount`
    /// only knows the old walker's `Owner` — pause/resume on a new-core
    /// embed is a documented no-op until a new-core visibility gate
    /// lands (same gap as `host-web`'s `WebHostHandle::pause`).
    pub fn pause(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.build_ui.is_none() {
            log::warn!("host-macos-desktop: pause() is a no-op on a new-core embed");
            return;
        }
        inner.host.unmount();
        drop(inner);
    }

    /// Re-mount the embedded app from its cached `build_ui`.
    /// Idempotent (no-op if already mounted). Pair with [`pause`].
    /// Old-core mounts only, exactly like [`pause`].
    pub fn resume(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.host.is_mounted() {
            return;
        }
        let Some(build_ui) = inner.build_ui.clone() else {
            return;
        };
        inner.host.mount(move || (&*build_ui)());
        // The swapchain may have been invalidated during the hidden
        // period; force a fresh one, else `get_current_texture`
        // returns Outdated/Lost for several frames and the canvas
        // stays at the clear color (see the iOS host's `resume`).
        inner.surface.configure(&inner.device, &inner.config);
    }

    /// True iff the embedded app is currently mounted.
    pub fn is_running(&self) -> bool {
        self.inner.borrow().host.is_mounted()
    }
}

/// Mount the wgpu render backend behind a framework `Graphics`
/// surface on macOS. Call from inside the surface's `on_ready`,
/// stash the returned handle so `on_resize` / `on_lost` can
/// reconfigure or drop it.
pub async fn mount(
    surface_handle: GraphicsSurface,
    size: (u32, u32),
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    build_ui: Rc<dyn Fn() -> Element + 'static>,
) -> Result<MacosHostHandle, MountError> {
    let init = init_wgpu(surface_handle, size).await?;

    // 3. Build the render-side stack + mount the user app. A fresh
    //    `session::REGISTRY` scope isolates the embedded app's
    //    `session::animated(…)` state to this host's lifetime, so a
    //    `LazyDisposing` navigator remount replays the embedded
    //    animation timeline from t=0 (see the iOS host for the full
    //    rationale).
    let session_scope = runtime_core::session::push_scope();
    let renderer = Renderer::new(&init.device, &init.queue, init.config.format);
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

    let (inner, render_loop_handle) =
        finish_mount(init, renderer, host, logical, Some(build_ui), session_scope);

    Ok(MacosHostHandle {
        #[cfg(feature = "new-core")]
        _newcore: None,
        inner,
        _render_loop: render_loop_handle,
    })
}

/// New-core sibling of [`mount`]: identical wgpu init, render loop, and
/// per-frame visibility gate, but the embedded tree realizes through
/// `render_wgpu::newcore::start_in_world` into the embedding APP's own
/// world (`backend_macos::newcore::mounted_world()`), so the app's
/// flush driver (backend-macos's dispatch-site wrappers + the
/// apple-core scheduler/executor post-dispatch hook) commits writes the
/// embedded app stages from timers, raf loops, and future polls — one
/// thread, one world, one logical update stream. The one-world-per-
/// thread argument from the web embed carries over verbatim:
/// `start_in_world` installs NO second dispatch hook and NO viewport
/// sink (an embedded sim must never clobber the page/app viewport —
/// regression-tested in `render-wgpu`'s newcore suite); the app host's
/// existing driver is the only committer.
///
/// Dropping the returned handle unrealizes the embedded scene (its
/// build-level effects, AV bind keepalives, and scoped timers die with
/// it — the `collect_owned` harvest inside `start_in_world`), then
/// tears the wgpu host down exactly like the old-core handle.
#[cfg(feature = "new-core")]
pub async fn mount_newcore(
    surface_handle: GraphicsSurface,
    size: (u32, u32),
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    build_ui: Rc<dyn Fn() -> runtime_scene::Element + 'static>,
) -> Result<MacosHostHandle, MountError> {
    // The app's world must exist BEFORE the async wgpu init runs —
    // fail fast on a mis-sequenced boot.
    let world = backend_macos::newcore::mounted_world().ok_or(MountError::NoHostWorld)?;
    // The per-frame loop below rides `runtime_core::driver::render_loop`.
    // `host-appkit` installs the NSTimer driver at boot on BOTH cores
    // (its `newcore::run` calls `backend_macos::install_render_loop`),
    // but the driver contract belongs to the shell, not this embed —
    // install idempotently (first install wins) so a host shell that
    // boots backend-macos directly still gets a painting embed, exactly
    // like `host_web::mount_newcore`'s `backend_web::install_render_loop`.
    backend_macos::install_render_loop();

    let init = init_wgpu(surface_handle, size).await?;

    // Same per-host session scope as the old mount (see the comment
    // there): the embedded app's `session::animated` AVs and epoch die
    // with this handle, so a remount replays from initial state instead
    // of resuming mid-animation.
    let session_scope = runtime_core::session::push_scope();
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
        // No re-callable builder: pause/resume are old-core-only (see
        // `MacosHostHandle::pause`); teardown goes through `_newcore`.
        finish_mount(init, renderer, host, logical, None, session_scope);

    Ok(MacosHostHandle {
        _newcore: Some(NewCoreGuard(Some(app))),
        inner,
        _render_loop: render_loop_handle,
    })
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Everything the async wgpu init produces — shared by [`mount`] and
/// [`mount_newcore`] (the host-web `WgpuInit` shape, plus the raw
/// `NSView*` the per-frame visibility walk needs).
struct WgpuInit {
    ns_view: *const NSObject,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

/// Steps 1–2 of both mounts: AppKit-handle validation + the async wgpu
/// init (instance → surface → adapter → device → configure). Metal
/// backend, same shape as host-ios-mobile.
async fn init_wgpu(
    surface_handle: GraphicsSurface,
    size: (u32, u32),
) -> Result<WgpuInit, MountError> {
    // 1. Validate the surface exposes an AppKit handle (see
    //    `backend-macos/src/imp/graphics.rs::MacosSurfaceProvider`).
    //    Capture the raw `NSView*` for the per-frame visibility check.
    let ns_view: *const NSObject = match surface_handle
        .window_handle()
        .map_err(|_| MountError::NoAppKitHandle)?
        .as_raw()
    {
        RawWindowHandle::AppKit(h) => h.ns_view.as_ptr() as *const NSObject,
        _ => return Err(MountError::NoAppKitHandle),
    };

    // 2. wgpu init — Metal backend, same shape as host-ios-mobile.
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
    // Clamp requested limits to what the adapter advertises — a no-op
    // on desktop Metal in practice, kept for parity with the iOS host
    // (where the Simulator's Metal reports lower caps).
    let adapter_limits = adapter.limits();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("host-macos-desktop-device"),
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
    // gamma encoding (same pick as host-web / host-ios-mobile).
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

    Ok(WgpuInit {
        ns_view,
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
    build_ui: Option<Rc<dyn Fn() -> Element + 'static>>,
    session_scope: runtime_core::session::ScopeGuard,
) -> (Rc<RefCell<HostInner>>, RenderLoop) {
    // 3a. macOS doesn't fetch pending font URLs (embedded faces
    //     cover the preview); log the count for diagnosis.
    let pending = host.take_pending_font_urls();
    if !pending.is_empty() {
        log::debug!(
            "host-macos-desktop: skipped fetch for {} pending font URL(s); \
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
        ns_view: init.ns_view,
        build_ui,
        presented_once: false,
        logged_acquire_skip: false,
        _session_scope: session_scope,
    }));

    // 4. Per-frame loop via the framework's render-loop driver — the
    //    NSTimer driver `host-appkit` installs at boot
    //    (`backend_macos::install_render_loop`).
    let inner_for_frame = inner.clone();
    let render_loop_handle = render_loop(move |_elapsed| {
        let mut inner = inner_for_frame.borrow_mut();
        draw_frame(&mut inner);
    });

    (inner, render_loop_handle)
}

/// Owns the embedded new-core app for a [`mount_newcore`] handle.
/// Drop routes through `render_wgpu::newcore::NewCoreApp::stop`
/// (the embedded path: unrealize + guarded diagnostic clear, app-host
/// flush driver untouched — a replacement embed may already have
/// mounted).
#[cfg(feature = "new-core")]
struct NewCoreGuard(Option<render_wgpu::newcore::NewCoreApp>);

#[cfg(feature = "new-core")]
impl Drop for NewCoreGuard {
    fn drop(&mut self) {
        if let Some(app) = self.0.take() {
            app.stop();
        }
    }
}

/// Restore the frame-active flag when the host dies. `draw_frame`
/// publishes `set_frame_active(visible)` every tick; if the host is
/// torn down while its view is hidden (navigating away from the
/// screen embedding it), the flag would otherwise stay `false`
/// forever and every author `raf_loop_scoped` ticker that reads
/// `is_frame_active()` stays frozen app-wide.
impl Drop for HostInner {
    fn drop(&mut self) {
        runtime_core::set_frame_active(true);
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
    /// Raw `NSView*` for the MetalView this host renders into.
    /// Checked each frame via `is_view_visible` so we skip the Metal
    /// command-buffer encode when the view is hidden behind a
    /// navigator's persistent-but-not-visible screen. Lifetime
    /// contract matches the iOS host: the framework's `Graphics`
    /// callbacks hold the `Slot<HostHandle>` that owns this
    /// `HostInner`, so while `HostInner` exists, the view exists.
    ns_view: *const NSObject,
    /// Re-callable embedded-app builder, cached for [`MacosHostHandle::resume`].
    /// `None` on a [`mount_newcore`] handle (pause/resume are
    /// old-core-only — see those methods).
    build_ui: Option<Rc<dyn Fn() -> Element + 'static>>,
    /// One-shot "first frame presented" diagnostic flag. The robot
    /// screenshot verb can't capture a framebuffer-only CAMetalLayer
    /// drawable, so this log line is the scriptable evidence that the
    /// host actually encoded + presented a frame.
    presented_once: bool,
    /// One-shot "acquire skipped before first present" diagnostic flag
    /// (see the Occluded arm in [`draw_frame`]).
    logged_acquire_skip: bool,
    /// RAII guard for this host's `session::REGISTRY` scope. Declared
    /// LAST so it pops AFTER the renderer / host / wgpu objects drop
    /// (their cleanups may dispatch through scope-anchored timers).
    _session_scope: runtime_core::session::ScopeGuard,
}

/// Walk the NSView chain checking `window != nil` and that no
/// ancestor is hidden / fully-transparent. NSView's selector is
/// `alphaValue` (UIView's is `alpha`) — both CGFloat.
unsafe fn is_view_visible(view: *const NSObject) -> bool {
    if view.is_null() {
        return false;
    }
    let window: *const NSObject = msg_send![view, window];
    if window.is_null() {
        return false;
    }
    let mut cur = view;
    loop {
        let hidden: bool = msg_send![cur, isHidden];
        if hidden {
            return false;
        }
        let alpha: f64 = msg_send![cur, alphaValue];
        if alpha <= 0.0 {
            return false;
        }
        let parent: *const NSObject = msg_send![cur, superview];
        if parent.is_null() {
            break;
        }
        cur = parent;
    }
    true
}

fn draw_frame(inner: &mut HostInner) {
    // Visibility gate — skip the GPU encode + present when the view
    // is hidden/off-window. Same policy as the iOS host: no
    // auto-unmount here (whether a hidden embedded app keeps running
    // is the caller's policy via pause/resume).
    let visible = unsafe { is_view_visible(inner.ns_view) };
    // Publish to the per-thread frame-active flag so author-side
    // `raf_loop_scoped` tickers that read `runtime_core::is_frame_active()`
    // can short-circuit while nothing paints.
    runtime_core::set_frame_active(visible);
    if !visible {
        return;
    }
    // wgpu 29: reconfigure on Outdated/Lost; skip on Timeout/
    // Occluded/Validation — same handling as the web and iOS hosts.
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
        | wgpu::CurrentSurfaceTexture::Validation => {
            // `Occluded` here does NOT mean the NSView failed the
            // visibility walk above — wgpu's Metal backend skips
            // drawable acquisition entirely (upstream workaround for
            // wgpu#8309's nextDrawable hang) whenever the NSWindow's
            // `occlusionState` lacks the on-screen bit: window behind
            // a fullscreen app, display asleep, locked session. The
            // embed keeps running (ticks, flushes) and presents as
            // soon as the window is actually on screen. One-shot log
            // so a "preview never paints" report is diagnosable from
            // stderr.
            if !inner.presented_once && !inner.logged_acquire_skip {
                inner.logged_acquire_skip = true;
                eprintln!(
                    "[host-macos-desktop] frames skipped before first present \
                     (surface not ready or window occluded — wgpu presents only \
                     while NSWindow.occlusionState reports on-screen)"
                );
            }
            return;
        }
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
    if !inner.presented_once {
        inner.presented_once = true;
        eprintln!(
            "[host-macos-desktop] first frame presented ({}x{} px, logical {}x{})",
            inner.config.width, inner.config.height, inner.logical.0, inner.logical.1
        );
    }
    // Advance per-frame state (animations, spinners, momentum). The
    // NSTimer keeps firing regardless of the return value.
    let _ = inner.host.tick();
}
