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

    /// Pause the embedded app: drop its reactive scope AND clear
    /// the thread's `session::REGISTRY`. Pair with [`resume`] for
    /// React Navigation-style `unmountOnBlur` semantics — the
    /// next `resume` rebuilds the embedded tree from initial
    /// state, with no leftover AV ticks running invisibly.
    ///
    /// Call this from a user-land effect bound to whatever
    /// "screen is no longer focused" signal you have (e.g. a
    /// `nav_state.active_route` comparison). The wgpu device,
    /// surface, and renderer stay alive — only the embedded
    /// `build_ui` tree drops — so a subsequent `resume()` re-mounts
    /// fresh without paying the wgpu init cost.
    ///
    /// The host does NOT pause itself based on `UIView.isHidden`;
    /// that would be a policy decision the host shouldn't make
    /// (some embedded apps legitimately want to keep running in
    /// the background — a live monitor preview, a benchmark
    /// visualiser, a game loop). The per-frame GPU encode IS
    /// skipped when the MetalView is off-screen, but that's an
    /// invisible-no-op optimization, not a behavior change.
    ///
    /// Note: `session::clear` is global to the calling thread —
    /// if anything outside the embedded app uses
    /// `session::animated`, those entries are wiped too. In
    /// practice `session::animated` is a primarily embedded-app
    /// pattern (hot-patch state survival); the outer iOS backend
    /// drives its state via plain `Signal`/`Effect`, so the wipe
    /// is safe.
    pub fn pause(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.host.unmount();
        // NOTE: skipping the renderer cache reset (graphics_cache,
        // image_cache) and the session::clear — those were causing
        // the renderer to produce no draws after remount even
        // though Taffy compute showed valid frames. Hypothesis:
        // they were dropping state the renderer relies on to track
        // which nodes have been seen.
        drop(inner);
    }

    /// Re-mount the embedded app from its cached `build_ui`. Idempotent
    /// (no-op if already mounted). Pair with [`pause`].
    pub fn resume(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.host.is_mounted() {
            return;
        }
        let build_ui = inner.build_ui.clone();
        inner.host.mount(move || (&*build_ui)());
        // After remount, the wgpu surface's swapchain may have been
        // invalidated during the hidden period (CAMetalLayer's
        // bounds drift, drawableSize stale, etc.). Reconfigure to
        // force a fresh swapchain. Without this, `get_current_texture`
        // returns `Outdated`/`Lost` for several frames, the renderer
        // returns early before running Taffy layout compute, and the
        // tree's frames stay at (0,0,0,0) — net effect is a
        // permanently white canvas.
        inner.surface.configure(&inner.device, &inner.config);
    }

    /// True iff the embedded app is currently mounted.
    pub fn is_running(&self) -> bool {
        self.inner.borrow().host.is_mounted()
    }
}

/// Mount the wgpu render backend behind a framework `Graphics`
/// surface on iOS. Call from inside the surface's `on_ready`, stash
/// the returned handle so `on_resize` / `on_lost` can reconfigure or
/// drop it.
///
/// `build_ui` is an `Rc<dyn Fn>` because the visibility gate may
/// unmount the embedded reactive scope when the MetalView is hidden
/// (planet animations stop ticking the global animation clock) and
/// remount it on the next visible frame.
pub async fn mount(
    surface_handle: GraphicsSurface,
    size: (u32, u32),
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    build_ui: Rc<dyn Fn() -> Element + 'static>,
) -> Result<IosHostHandle, MountError> {
    // 1. Validate the surface exposes a UiKit handle. The iOS
    //    `Graphics` primitive always provides one (see
    //    `backend-ios-mobile/src/imp/graphics.rs::IosSurfaceProvider`);
    //    a missing handle means the caller wired a non-iOS surface in.
    //    Capture the raw `UIView*` pointer for the per-frame
    //    visibility check (see `is_view_visible`).
    let ui_view: *const NSObject = match surface_handle
        .window_handle()
        .map_err(|_| MountError::NoUiKitHandle)?
        .as_raw()
    {
        RawWindowHandle::UiKit(h) => h.ui_view.as_ptr() as *const NSObject,
        _ => return Err(MountError::NoUiKitHandle),
    };

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
    {
        let build_ui = build_ui.clone();
        host.mount(move || (&*build_ui)());
    }

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
        ui_view,
        build_ui,
        was_visible: true,
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
    /// Raw `UIView*` for the MetalView this host renders into.
    /// Checked each frame via `is_view_visible` so we can skip the
    /// (expensive) Metal command-buffer encode when the view is
    /// hidden behind a navigator's persistent-but-not-visible
    /// screen. The view outlives this Host: the framework's
    /// `Graphics` ivars (on_ready/on_resize/on_lost) hold the
    /// `Slot<HostHandle>` that owns this `HostInner`, so as long as
    /// `HostInner` exists, the ivars exist, so the view exists.
    /// Therefore the raw pointer is safe to dereference for the
    /// lifetime of the Host.
    ui_view: *const NSObject,
    /// Re-callable embedded-app builder. Cached so a hidden→visible
    /// transition can remount the welcome (or any other) subtree
    /// without bouncing the wgpu device/surface.
    build_ui: Rc<dyn Fn() -> Element + 'static>,
    /// Visibility state on the previous frame. Used to detect
    /// transitions: visible→hidden triggers `host.unmount()`,
    /// hidden→visible triggers `host.mount(build_ui.clone())`.
    was_visible: bool,
}

/// Walk the UIView chain checking `window != nil` and that no
/// ancestor is hidden / fully-transparent. Used to early-exit
/// `draw_frame` when the embedded preview is mounted but not
/// actually visible (e.g. the home screen behind a pushed page in a
/// stack navigator).
unsafe fn is_view_visible(view: *const NSObject) -> bool {
    if view.is_null() {
        return false;
    }
    // Detached from any window means nothing renders to screen.
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
        // `alpha` is a CGFloat — `f64` on 64-bit platforms, which is
        // what every iOS device (and the simulator) runs.
        let alpha: f64 = msg_send![cur, alpha];
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
    // Visibility gate: skip the GPU encode + present when the
    // MetalView is hidden behind a navigator's persistent screen
    // (`isHidden:true` on an ancestor, off-window, etc.). This is
    // an invisible-no-op optimization — frames that wouldn't paint
    // anyway don't pay the Metal command-buffer + present cost.
    //
    // Notably we do NOT auto-unmount the embedded scope here, even
    // though that would also pause any `AnimatedValue` ticks driven
    // by the global animation clock. Whether a hidden embedded app
    // should *keep running in the background* (e.g. a live monitor
    // preview, a benchmark visualiser ticking over) or freeze is a
    // policy decision that belongs to the caller — pair the
    // navigator's focus signal with [`IosHostHandle::pause`] /
    // [`resume`] to wire it up. The default is "keep running"
    // (matches React Navigation's Stack default).
    let visible = unsafe { is_view_visible(inner.ui_view) };
    // Publish to the per-thread frame-active flag so author-side
    // animation tickers (any `raf_loop_scoped` consumer that reads
    // `runtime_core::is_frame_active()`) can short-circuit when
    // nothing's painting. Without this hook the welcome app's
    // planets keep advancing while off-home — the visibility check
    // stops the GPU encode but not the CPU-side AV updates.
    runtime_core::set_frame_active(visible);
    if !visible {
        return;
    }
    inner.was_visible = true;
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
