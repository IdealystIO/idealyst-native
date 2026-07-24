//! Windows-only implementation of [`crate::mount`]. Mirrors
//! `host-macos-desktop::macos` end-to-end with two deltas:
//!
//! 1. The surface is a DirectComposition VISUAL (see
//!    `backend-windows::dcomp`), not a window/view handle — wgpu
//!    mounts on it via `SurfaceTargetUnsafe::CompositionVisual` and
//!    the wgpu backend is DX12 instead of Metal. Two contract points
//!    the visual path adds: the alpha mode must be explicit (`Opaque`;
//!    composition swapchains reject the caps-default `Auto` →
//!    DXGI_ALPHA_MODE_UNSPECIFIED), and every `surface.configure`
//!    must be followed by a composition-device commit
//!    ([`SurfaceAnchor::commit_after_configure`]) or the bound
//!    swapchain never appears. A plain Win32 window handle remains as
//!    a fallback anchor. Same adapter-limits clamp (harmless on
//!    desktop — the adapter advertises more than `downlevel_defaults`
//!    asks for).
//! 2. Visibility comes from the anchor: the backend's live
//!    `ComposedTarget::is_visible` flag for visuals (portal-hidden ⇒
//!    false), or one `IsWindowVisible` call for the HWND fallback —
//!    replacing the per-ancestor `isHidden`/`alphaValue` NSView walk.
//!
//! Font behavior matches macOS: no fetching — `face!` fonts are
//! embedded into the binary by the `embed-font-bytes` feature, so
//! the wgpu Host's cosmic-text shaper falls back to its built-in
//! default face when the registered fonts aren't bytes-backed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use render_api::DeviceProfile;
use render_wgpu::{Host, Painter, Renderer};
use runtime_core::driver::{render_loop, RenderLoop};
use runtime_core::primitives::graphics::{ComposedTarget, GraphicsSurface};
use runtime_core::Element;

use std::sync::Arc;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{IsWindow, IsWindowVisible};

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum MountError {
    /// `GraphicsSurface` exposes neither a composition visual nor a
    /// Win32 window handle. The Windows `Graphics` primitive always
    /// provides the former, so this should only fire on a misuse
    /// (e.g. handing in a web `CanvasSurfaceProvider`).
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
        inner.anchor.commit_after_configure();
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
        inner.anchor.commit_after_configure();
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
    // 1. Resolve where the swapchain lives. Preferred: the surface's
    //    composition-visual capability (the Windows backend's graphics
    //    surfaces are DirectComposition visuals — see
    //    `backend-windows::dcomp`); wgpu mounts on the visual directly.
    //    Fallback: a real Win32 window handle (kept for any host that
    //    still hands one in). The anchor is stored for the per-frame
    //    visibility check.
    let anchor: SurfaceAnchor = match surface_handle.composed_target() {
        Some(ct) => SurfaceAnchor::Composed(ct.clone()),
        None => match surface_handle.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => SurfaceAnchor::Window(h.hwnd.get()),
            _ => return Err(MountError::NoWin32Handle),
        },
    };

    // 2. wgpu init — DX12 backend, same shape as host-macos-desktop.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
        flags: wgpu::InstanceFlags::empty(),
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });
    let surface = match &anchor {
        SurfaceAnchor::Composed(ct) => unsafe {
            // SAFETY: the visual is a live IDCompositionVisual whose
            // reference the provider (held via `anchor` in HostInner)
            // owns for this host's whole lifetime; wgpu additionally
            // AddRefs it internally.
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(
                ct.visual().as_ptr(),
            ))
        }
        .map_err(|_| MountError::CreateSurface)?,
        SurfaceAnchor::Window(_) => instance
            .create_surface(surface_handle)
            .map_err(|_| MountError::CreateSurface)?,
    };
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
    // Alpha: on a composition target the caps list leads with `Auto`,
    // which DX12 maps to DXGI_ALPHA_MODE_UNSPECIFIED — and
    // `CreateSwapChainForComposition` REJECTS unspecified alpha, so the
    // swapchain would fail at first configure. Pick `Opaque`
    // explicitly (→ DXGI_ALPHA_MODE_IGNORE, valid for composition, and
    // the canvas is opaque anyway). The HWND path keeps the caps
    // default as before.
    let alpha_mode = match &anchor {
        SurfaceAnchor::Composed(_) => wgpu::CompositeAlphaMode::Opaque,
        SurfaceAnchor::Window(_) => caps.alpha_modes[0],
    };
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.0.max(1),
        height: size.1.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode,
        view_formats: vec![],
    };
    surface.configure(&device, &config);
    // Composition contract: wgpu's configure binds the new swapchain to
    // the visual (`SetContent`) but cannot commit — the composition
    // device is the backend's. Without this commit the canvas stays
    // invisible until some unrelated layout/scroll change commits.
    anchor.commit_after_configure();

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
        anchor,
        host_id: NEXT_HOST_ID.fetch_add(1, Ordering::Relaxed),
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

// ---------------------------------------------------------------------------
// Frame-active aggregation across hosts
// ---------------------------------------------------------------------------
//
// `runtime_core::set_frame_active` is ONE thread-local flag, but this
// thread can run SEVERAL hosts at once: the website's swap navigator
// keeps the home screen (and its Simulator host) MOUNTED and merely
// portal-hides it on navigation, so returning home via a fresh route
// instance leaves a hidden host + a visible host ticking side by side.
// If each published its own visibility directly, the hidden one
// stomped `false` every frame and the visible Simulator's timeline
// froze at t=0 (the "site loads then the demo freezes" bug). Instead
// every host VOTES its visibility here and the flag is the aggregate:
// any-visible, or `true` when no hosts are alive (the framework
// default, so author tickers never stay frozen after teardown).

thread_local! {
    static HOST_VISIBILITY: RefCell<HashMap<u64, bool>> = RefCell::new(HashMap::new());
}

static NEXT_HOST_ID: AtomicU64 = AtomicU64::new(1);

/// The aggregate the votes imply: any visible host, or `true` for an
/// empty map. Pure — unit-tested below.
fn aggregate_frame_active(votes: &HashMap<u64, bool>) -> bool {
    votes.is_empty() || votes.values().any(|v| *v)
}

/// Record `visible` for host `id` and publish the aggregate.
fn publish_visibility(id: u64, visible: bool) {
    let agg = HOST_VISIBILITY.with(|m| {
        let mut m = m.borrow_mut();
        m.insert(id, visible);
        aggregate_frame_active(&m)
    });
    runtime_core::set_frame_active(agg);
}

/// Drop host `id`'s vote and publish the aggregate.
fn retire_visibility(id: u64) {
    let agg = HOST_VISIBILITY.with(|m| {
        let mut m = m.borrow_mut();
        m.remove(&id);
        aggregate_frame_active(&m)
    });
    runtime_core::set_frame_active(agg);
}

/// Retire this host's visibility vote when it dies, restoring the
/// aggregate (or the `true` default once no hosts remain) so author
/// `raf_loop_scoped` tickers never stay frozen after teardown.
impl Drop for HostInner {
    fn drop(&mut self) {
        retire_visibility(self.host_id);
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
    /// What the swapchain is mounted on — checked each frame for
    /// visibility so we skip the GPU encode when the surface is hidden
    /// behind a navigator's persistent-but-not-visible screen. The
    /// `Composed` variant also keeps the backend's provider (and thus
    /// the composition visual) alive for this host's whole lifetime.
    anchor: SurfaceAnchor,
    /// This host's key in the thread's `HOST_VISIBILITY` vote map.
    host_id: u64,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The website freeze: navigating away portal-hides the home
    /// screen (its Simulator host stays mounted), returning mounts a
    /// second host. With one shared flag, the hidden host's every-tick
    /// `false` froze the visible host's timeline at t=0. The
    /// aggregate must be any-visible; and with no hosts at all it
    /// must be `true` (the framework default) so tickers never stay
    /// frozen after teardown.
    #[test]
    fn regression_hidden_host_does_not_freeze_visible_host() {
        let mut votes = HashMap::new();
        assert!(aggregate_frame_active(&votes), "no hosts = default active");
        votes.insert(1, false); // hidden home-screen host
        votes.insert(2, true); // visible remounted host
        assert!(aggregate_frame_active(&votes), "one visible host keeps frames active");
        votes.insert(2, false);
        assert!(!aggregate_frame_active(&votes), "all hidden = paused");
        votes.remove(&1);
        votes.remove(&2);
        assert!(aggregate_frame_active(&votes), "teardown restores the default");
    }

    struct FakeComposed {
        visible: std::sync::atomic::AtomicBool,
        commits: std::sync::atomic::AtomicU32,
    }

    impl ComposedTarget for FakeComposed {
        fn visual(&self) -> std::ptr::NonNull<std::ffi::c_void> {
            std::ptr::NonNull::new(0x1 as *mut std::ffi::c_void).unwrap()
        }
        fn commit(&self) {
            self.commits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        fn is_visible(&self) -> bool {
            self.visible.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// The composed anchor must relay the backend's LIVE visibility
    /// flag (the frame gate + the multi-host votes both read it), and
    /// `commit_after_configure` must reach the backend's composition
    /// device — a swapchain bound by `configure` is invisible until
    /// that commit, so a swallowed call = permanently blank canvas.
    #[test]
    fn composed_anchor_relays_visibility_and_commits() {
        let ct = Arc::new(FakeComposed {
            visible: std::sync::atomic::AtomicBool::new(true),
            commits: std::sync::atomic::AtomicU32::new(0),
        });
        let anchor = SurfaceAnchor::Composed(ct.clone());
        assert!(anchor.is_visible());
        ct.visible.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!anchor.is_visible(), "anchor must observe live changes");
        anchor.commit_after_configure();
        anchor.commit_after_configure();
        assert_eq!(ct.commits.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}

/// Where this host's swapchain is mounted: a composition visual (the
/// Windows backend's graphics surfaces) or a real window handle (the
/// legacy fallback).
enum SurfaceAnchor {
    Composed(Arc<dyn ComposedTarget>),
    Window(isize),
}

impl SurfaceAnchor {
    /// Whether the surface currently shows anything. Composed: the
    /// backend's live flag (portal-hidden ⇒ false). Window:
    /// `IsWindowVisible` ANDs the WS_VISIBLE bit up the entire parent
    /// chain, and `IsWindow` catches a destroyed handle during
    /// teardown races.
    fn is_visible(&self) -> bool {
        match self {
            SurfaceAnchor::Composed(ct) => ct.is_visible(),
            SurfaceAnchor::Window(hwnd) => {
                let h = HWND(*hwnd as *mut c_void);
                unsafe { IsWindow(h).as_bool() && IsWindowVisible(h).as_bool() }
            }
        }
    }

    /// A swapchain (re)bound to a composition visual only appears at
    /// the next composition-device commit — call after every
    /// `surface.configure`. No-op for a window-mounted swapchain.
    fn commit_after_configure(&self) {
        if let SurfaceAnchor::Composed(ct) = self {
            ct.commit();
        }
    }
}

fn draw_frame(inner: &mut HostInner) {
    // Visibility gate — skip the GPU encode + present when the surface
    // is hidden. Same policy as the other hosts: no auto-unmount here
    // (whether a hidden embedded app keeps running is the caller's
    // policy via pause/resume).
    let visible = inner.anchor.is_visible();
    // Vote this host's visibility into the per-thread aggregate (see
    // `HOST_VISIBILITY`) so author-side `raf_loop_scoped` tickers that
    // read `runtime_core::is_frame_active()` can short-circuit while
    // nothing paints — without a hidden sibling host freezing them.
    publish_visibility(inner.host_id, visible);
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
            inner.anchor.commit_after_configure();
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
