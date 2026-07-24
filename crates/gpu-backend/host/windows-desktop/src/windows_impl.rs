//! Windows-only implementation of [`crate::mount`]. Mirrors
//! `host-macos-desktop::macos` end-to-end with two deltas:
//!
//! 1. The surface exposes a Win32 window handle (`hwnd`), not an
//!    AppKit one, and the wgpu backend is DX12 instead of Metal.
//!    Same adapter-limits clamp (harmless on desktop — the adapter
//!    advertises more than `downlevel_defaults` asks for).
//! 2. The visibility walk is one Win32 call: `IsWindowVisible`
//!    already checks the WS_VISIBLE bit up the whole parent chain
//!    (plus `IsWindow` for a destroyed handle), replacing the
//!    per-ancestor `isHidden`/`alphaValue` NSView walk.
//!
//! Font behavior matches macOS: no fetching — `face!` fonts are
//! embedded into the binary by the `embed-font-bytes` feature, so
//! the wgpu Host's cosmic-text shaper falls back to its built-in
//! default face when the registered fonts aren't bytes-backed.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use render_api::DeviceProfile;
use render_wgpu::{Host, Painter, Renderer};
use runtime_core::driver::{render_loop, RenderLoop};
use runtime_core::primitives::graphics::GraphicsSurface;
use runtime_core::Element;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{IsWindow, IsWindowVisible};

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum MountError {
    /// `GraphicsSurface` doesn't expose a Win32 window handle. The
    /// Windows `Graphics` primitive always provides one, so this
    /// should only fire on a misuse (e.g. handing in a web
    /// `CanvasSurfaceProvider`).
    NoWin32Handle,
    /// `wgpu::Instance::create_surface` rejected the handle.
    CreateSurface,
    /// `wgpu::Instance::request_adapter` returned no DX12 adapter.
    NoAdapter,
    /// `Adapter::request_device` rejected the limits we asked for.
    RequestDevice,
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MountError::NoWin32Handle => {
                write!(f, "host-windows-desktop: GraphicsSurface has no Win32 window handle")
            }
            MountError::CreateSurface => {
                write!(f, "host-windows-desktop: wgpu create_surface failed")
            }
            MountError::NoAdapter => {
                write!(f, "host-windows-desktop: no compatible DX12 adapter")
            }
            MountError::RequestDevice => {
                write!(f, "host-windows-desktop: wgpu request_device failed")
            }
        }
    }
}

impl std::error::Error for MountError {}

/// Live handle for one embedded preview. Drop it to release the wgpu
/// device / queue / surface and cancel the render loop. `!Send +
/// !Sync` because the interior state is single-threaded (Rc, wgpu
/// objects, the render-loop guard).
pub struct WindowsHostHandle {
    inner: Rc<RefCell<HostInner>>,
    /// Holding the handle keeps the per-frame closure alive; drop =
    /// cancel the raf subscription. Declared LAST so the loop survives
    /// long enough for `inner`'s Rc clones inside the closure to drop.
    _render_loop: RenderLoop,
}

impl WindowsHostHandle {
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
    /// [`Self::resume`] for `unmountOnBlur` semantics. The wgpu
    /// device, surface, and renderer stay alive — only the embedded
    /// `build_ui` tree drops — so a subsequent `resume()` re-mounts
    /// fresh without paying the wgpu init cost.
    pub fn pause(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.host.unmount();
        drop(inner);
    }

    /// Re-mount the embedded app from its cached `build_ui`.
    /// Idempotent (no-op if already mounted). Pair with [`Self::pause`].
    pub fn resume(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.host.is_mounted() {
            return;
        }
        let build_ui = inner.build_ui.clone();
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
/// surface on Windows. Call from inside the surface's `on_ready`,
/// stash the returned handle so `on_resize` / `on_lost` can
/// reconfigure or drop it.
pub async fn mount(
    surface_handle: GraphicsSurface,
    size: (u32, u32),
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    build_ui: Rc<dyn Fn() -> Element + 'static>,
) -> Result<WindowsHostHandle, MountError> {
    // 1. Validate the surface exposes a Win32 handle (see
    //    `backend-windows::graphics::Win32SurfaceProvider`). Capture
    //    the raw HWND for the per-frame visibility check.
    let hwnd: isize = match surface_handle
        .window_handle()
        .map_err(|_| MountError::NoWin32Handle)?
        .as_raw()
    {
        RawWindowHandle::Win32(h) => h.hwnd.get(),
        _ => return Err(MountError::NoWin32Handle),
    };

    // 2. wgpu init — DX12 backend, same shape as host-macos-desktop.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
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
    // on desktop DX12 in practice, kept for parity with the mobile
    // hosts (where Metal/GLES report lower caps).
    let adapter_limits = adapter.limits();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("host-windows-desktop-device"),
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
    // gamma encoding (same pick as the other hosts).
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

    // 3. Build the render-side stack + mount the user app. A fresh
    //    `session::REGISTRY` scope isolates the embedded app's
    //    `session::animated(…)` state to this host's lifetime, so a
    //    `LazyDisposing` navigator remount replays the embedded
    //    animation timeline from t=0 (see the iOS host for the full
    //    rationale).
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

    // 3a. Windows doesn't fetch pending font URLs (embedded faces
    //     cover the preview); log the count for diagnosis.
    let pending = host.take_pending_font_urls();
    if !pending.is_empty() {
        log::debug!(
            "host-windows-desktop: skipped fetch for {} pending font URL(s); \
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
        hwnd,
        build_ui,
        presented_once: false,
        _session_scope: session_scope,
    }));

    // 4. Per-frame loop via the framework's render-loop driver — the
    //    raf-backed driver `host-win32` installs at boot alongside its
    //    scheduler.
    let inner_for_frame = inner.clone();
    let render_loop_handle = render_loop(move |_elapsed| {
        let mut inner = inner_for_frame.borrow_mut();
        draw_frame(&mut inner);
    });

    Ok(WindowsHostHandle {
        inner,
        _render_loop: render_loop_handle,
    })
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Restore the frame-active flag when the host dies. `draw_frame`
/// publishes `set_frame_active(visible)` every tick; if the host is
/// torn down while its surface is hidden (navigating away from the
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
    /// Raw HWND (as isize) of the child window this host renders
    /// into. Checked each frame via `is_surface_visible` so we skip
    /// the GPU encode when the window is hidden behind a navigator's
    /// persistent-but-not-visible screen. Lifetime contract matches
    /// the other hosts: the framework's `Graphics` callbacks hold the
    /// `Slot<HostHandle>` that owns this `HostInner`, so while
    /// `HostInner` exists, the HWND exists.
    hwnd: isize,
    /// Re-callable embedded-app builder, cached for [`WindowsHostHandle::resume`].
    build_ui: Rc<dyn Fn() -> Element + 'static>,
    /// One-shot "first frame presented" diagnostic flag — scriptable
    /// evidence on stderr that the host actually encoded + presented.
    presented_once: bool,
    /// RAII guard for this host's `session::REGISTRY` scope. Declared
    /// LAST so it pops AFTER the renderer / host / wgpu objects drop
    /// (their cleanups may dispatch through scope-anchored timers).
    _session_scope: runtime_core::session::ScopeGuard,
}

/// Whether the target HWND is live and visible. `IsWindowVisible`
/// already ANDs the WS_VISIBLE bit up the entire parent chain (so a
/// `set_portal_hidden` ShowWindow(SW_HIDE) on any ancestor gates the
/// render), and `IsWindow` catches a destroyed handle during teardown
/// races.
fn is_surface_visible(hwnd: isize) -> bool {
    let h = HWND(hwnd as *mut c_void);
    unsafe { IsWindow(h).as_bool() && IsWindowVisible(h).as_bool() }
}

fn draw_frame(inner: &mut HostInner) {
    // Visibility gate — skip the GPU encode + present when the window
    // is hidden. Same policy as the other hosts: no auto-unmount here
    // (whether a hidden embedded app keeps running is the caller's
    // policy via pause/resume).
    let visible = is_surface_visible(inner.hwnd);
    // Publish to the per-thread frame-active flag so author-side
    // `raf_loop_scoped` tickers that read `runtime_core::is_frame_active()`
    // can short-circuit while nothing paints.
    runtime_core::set_frame_active(visible);
    if !visible {
        return;
    }
    // wgpu 29: reconfigure on Outdated/Lost; skip on Timeout/
    // Occluded/Validation — same handling as the web and mobile hosts.
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
    if !inner.presented_once {
        inner.presented_once = true;
        eprintln!(
            "[host-windows-desktop] first frame presented ({}x{} px, logical {}x{})",
            inner.config.width, inner.config.height, inner.logical.0, inner.logical.1
        );
    }
    // Advance per-frame state (animations, spinners, momentum). The
    // raf pulse keeps firing regardless of the return value.
    let _ = inner.host.tick();
}
