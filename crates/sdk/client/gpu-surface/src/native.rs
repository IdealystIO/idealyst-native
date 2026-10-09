//! Native (non-wasm) surface bring-up and mount lifecycle.
//!
//! Two kinds of `graphics` target reach a renderer:
//!
//! - **RawWindow** (macOS/iOS/Android/Windows): a window handle wgpu turns into
//!   a swapchain surface. The frame target is blitted into the acquired
//!   surface texture and presented.
//! - **Gl** (Linux/GTK4): GTK4 has no per-widget native window, so the
//!   primitive lends a live GL context (`GtkGLArea`). It is adopted through
//!   wgpu's GL backend, and the frame target is flip-blitted into the area's
//!   framebuffer from INSIDE GTK's render signal ([`GpuSurface::blit_to_fbo`]).

use crate::config::{check_adapter, choose_alpha_mode, choose_surface_format, AdapterFacts};
use crate::present::{surface_wants_premultiplied, PresentBlit};
use crate::{make_target, FrameAlpha, Requirements};
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::graphics::{GraphicsTarget, OnReadyEvent, OnResizeEvent};
use runtime_vocabulary::caps::GraphicsOps;

use std::cell::RefCell;
use std::rc::Rc;

/// Does the default (headless) adapter meet `req`? The register-time gate a
/// renderer uses to decide whether to install itself at all, so a fallback
/// renderer registered earlier keeps the payload on GPUs that can't run it.
/// Logs one line naming the missing capability when it refuses.
pub fn adapter_meets_default(req: Requirements, label: &str) -> bool {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    })) else {
        log::warn!("{label}: no GPU adapter — not registering");
        return false;
    };
    match check_adapter(AdapterFacts::of(&adapter), req) {
        Ok(()) => true,
        Err(why) => {
            let info = adapter.get_info();
            log::warn!("{label}: {:?} adapter {:?} {why} — not registering", info.backend, info.name);
            false
        }
    }
}

/// A ready GPU for one `graphics` surface: device + queue, the present path,
/// and the frame target a renderer draws into.
///
/// `device` and `queue` are public fields (not accessors) so a renderer that
/// owns a `GpuSurface` alongside its own GPU objects can borrow them
/// disjointly in one expression.
pub struct GpuSurface {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// The adapter the device came from — for capability queries
    /// (multisample support per format, downlevel flags, backend name).
    adapter: wgpu::Adapter,
    /// Swapchain surface (RawWindow targets); `None` on the GL path.
    surface: Option<wgpu::Surface<'static>>,
    /// GL present path (GTK4); `Some` exactly when `surface` is `None`.
    #[cfg(target_os = "linux")]
    gl: Option<GlPresent>,
    /// Swapchain config; on the GL path only the size/format book (never used
    /// to configure anything — GTK owns the buffers).
    config: wgpu::SurfaceConfiguration,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    present: PresentBlit,
    scale: f64,
    label: &'static str,
}

/// A frame acquired by [`GpuSurface::begin_present`], handed back to
/// [`GpuSurface::finish_present`] after the encoder is submitted.
pub enum PresentFrame {
    Surface(wgpu::SurfaceTexture),
    /// GL (GTK4): nothing was blitted — the FBO blit happens in GTK's render
    /// signal; finishing only schedules that pass.
    #[cfg(target_os = "linux")]
    Gl,
}

impl GpuSurface {
    /// Bring up a device on `target`. `None` when there's no adapter, the
    /// adapter fails `req`, or (GL) the lent context can't be adopted.
    ///
    /// Blocking (`pollster`): the backends deliver `on_ready` on a deferred
    /// runloop turn, where blocking is safe.
    pub fn new(
        target: GraphicsTarget,
        size: (u32, u32),
        scale: f32,
        req: Requirements,
        frame_alpha: FrameAlpha,
        label: &'static str,
    ) -> Option<Self> {
        let (w, h) = (size.0.max(1), size.1.max(1));
        let scale = if scale > 0.0 { scale as f64 } else { 1.0 };
        match target {
            GraphicsTarget::RawWindow(surface_target) => {
                let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                    backends: wgpu::Backends::PRIMARY,
                    flags: wgpu::InstanceFlags::default(),
                    memory_budget_thresholds: Default::default(),
                    backend_options: wgpu::BackendOptions::default(),
                    // `None` lets wgpu fall back to the per-surface handle (only
                    // GLES/Wayland need an explicit display handle).
                    display: None,
                });
                // `GraphicsSurface` is 'static + Send + Sync and implements the
                // raw-window-handle traits → `Surface<'static>`.
                let surf = instance.create_surface(surface_target).ok()?;
                let adapter =
                    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::HighPerformance,
                        force_fallback_adapter: false,
                        compatible_surface: Some(&surf),
                    }))
                    .ok()?;
                if let Err(why) = check_adapter(AdapterFacts::of(&adapter), req) {
                    log::warn!("{label}: surface adapter {why}");
                    return None;
                }
                let (device, queue) = pollster::block_on(request_device(&adapter, label)).ok()?;

                let caps = surf.get_capabilities(&adapter);
                let format = choose_surface_format(&caps.formats)?;
                let alpha_mode = choose_alpha_mode(&caps.alpha_modes, false);
                let config = wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format,
                    width: w,
                    height: h,
                    present_mode: wgpu::PresentMode::AutoVsync,
                    desired_maximum_frame_latency: 2,
                    alpha_mode,
                    view_formats: vec![],
                };
                surf.configure(&device, &config);
                let present = PresentBlit::new(
                    &device,
                    format,
                    frame_alpha,
                    surface_wants_premultiplied(alpha_mode),
                    false,
                );
                let (target, target_view) = make_target(&device, w, h, label);
                Some(GpuSurface {
                    device,
                    queue,
                    adapter,
                    surface: Some(surf),
                    #[cfg(target_os = "linux")]
                    gl: None,
                    config,
                    target,
                    target_view,
                    present,
                    scale,
                    label,
                })
            }
            GraphicsTarget::Gl(gl_target) => {
                #[cfg(target_os = "linux")]
                {
                    // Contract: the context must be current for the adopt call and
                    // for every use/drop of what it returns (see `GlTarget` /
                    // `adopt`). render paths and Drop each re-establish it.
                    gl_target.make_current();
                    let (adapter, device, queue) = adopt(&gl_target, label)?;
                    // Scriptable evidence the GPU path engaged, independent of
                    // whether the embedding app installed a `log` backend
                    // (`host-gtk` installs none).
                    eprintln!("[{label}] adopted GtkGLArea GL context — GPU active ({w}x{h})");
                    // GTK composites GL areas as premultiplied, with a
                    // bottom-left origin.
                    let present =
                        PresentBlit::new(&device, FRAMEBUFFER_FORMAT, frame_alpha, true, true);
                    let config = wgpu::SurfaceConfiguration {
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        format: FRAMEBUFFER_FORMAT,
                        width: w,
                        height: h,
                        present_mode: wgpu::PresentMode::AutoVsync,
                        desired_maximum_frame_latency: 2,
                        alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
                        view_formats: vec![],
                    };
                    let (target, target_view) = make_target(&device, w, h, label);
                    Some(GpuSurface {
                        device,
                        queue,
                        adapter,
                        surface: None,
                        gl: Some(GlPresent { target: gl_target }),
                        config,
                        target,
                        target_view,
                        present,
                        scale,
                        label,
                    })
                }
                #[cfg(not(target_os = "linux"))]
                {
                    // Only GTK4 lends a GL context, and that backend only builds
                    // on Linux.
                    let _ = (gl_target, req, frame_alpha);
                    log::warn!("{label}: GL graphics target off Linux — unsupported");
                    None
                }
            }
        }
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

    /// Device pixel ratio (physical px per logical unit) from the graphics
    /// event. `1.0` when the backend doesn't report one.
    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn label(&self) -> &'static str {
        self.label
    }

    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    /// The frame target (`TARGET_FORMAT`, drawable-sized).
    pub fn target(&self) -> &wgpu::Texture {
        &self.target
    }

    pub fn target_view(&self) -> &wgpu::TextureView {
        &self.target_view
    }

    /// `true` on the adopted-GL path. A GL context is bound to the thread
    /// that made it current, so anything that compiles pipelines on worker
    /// threads (vello's parallel shader init) must stay single-threaded.
    pub fn is_gl(&self) -> bool {
        self.surface.is_none()
    }

    /// Make the lent GL context current (no-op on the swapchain path). Call
    /// before creating or dropping GPU objects outside the render paths.
    pub fn make_current(&self) {
        #[cfg(target_os = "linux")]
        if let Some(gl) = &self.gl {
            gl.target.make_current();
        }
    }

    /// Resize to `size` (physical px) at `scale`: reconfigures the swapchain
    /// and rebuilds the frame target. Size-dependent renderer resources are
    /// the caller's to drop (see [`SurfaceState::resized`]).
    pub fn resize(&mut self, size: (u32, u32), scale: f32) {
        if scale > 0.0 {
            self.scale = scale as f64;
        }
        self.config.width = size.0.max(1);
        self.config.height = size.1.max(1);
        self.make_current();
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
        let (target, target_view) =
            make_target(&self.device, self.config.width, self.config.height, self.label);
        self.target = target;
        self.target_view = target_view;
    }

    /// Acquire the present destination and record the frame-target blit into
    /// `encoder`. `None` when the swapchain texture can't be acquired (drawable
    /// not ready — common for a fresh `CAMetalLayer`'s first frame; or
    /// occluded/outdated/lost): skip the frame. On the GL path nothing is
    /// recorded (see [`PresentFrame::Gl`]).
    pub fn begin_present(&self, encoder: &mut wgpu::CommandEncoder) -> Option<PresentFrame> {
        match &self.surface {
            Some(surface) => {
                let frame = match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(t)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                    _ => return None,
                };
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                self.present.draw(&self.device, encoder, &self.target_view, &view);
                Some(PresentFrame::Surface(frame))
            }
            None => {
                #[cfg(target_os = "linux")]
                {
                    Some(PresentFrame::Gl)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    unreachable!("a non-Linux GpuSurface always has a swapchain surface")
                }
            }
        }
    }

    /// Present a frame from [`begin_present`](Self::begin_present). Call after
    /// submitting the encoder that recorded the blit.
    pub fn finish_present(&self, frame: PresentFrame) {
        match frame {
            PresentFrame::Surface(t) => t.present(),
            // GL (GTK4): tell the backend the framebuffer is ready so GTK runs
            // its render pass (queue_render), which calls `blit_to_fbo`.
            #[cfg(target_os = "linux")]
            PresentFrame::Gl => {
                if let Some(gl) = &self.gl {
                    gl.target.present();
                }
            }
        }
    }

    /// GL (GTK4) only: copy the last rendered frame target into the GLArea's
    /// framebuffer. Registered via `GlTarget::on_render` by [`mount`]; GTK
    /// calls it with the context current and the area's FBO bound.
    ///
    /// GtkGLArea composites ONLY what is drawn during its own render signal;
    /// the reactive render runs out-of-band and only schedules that pass. A
    /// blit done anywhere else is never shown (the "canvas stuck on the clear
    /// colour" bug).
    #[cfg(target_os = "linux")]
    pub fn blit_to_fbo(&self) {
        let Some(gl) = &self.gl else { return };
        // Cheap, and re-binds the FBO that `wrap_framebuffer` reads below.
        gl.target.make_current();
        // Re-wrapped every frame: GTK reallocates the attachment across
        // resizes and re-realizes; a cached wrap would target freed memory.
        let Some(fbo_tex) = wrap_framebuffer(&self.device, &gl.target, self.width(), self.height())
        else {
            return; // not wrappable yet (mid-realize); a later present re-runs this
        };
        let fbo_view = fbo_tex.create_view(&wgpu::TextureViewDescriptor::default());
        debug_assert_eq!(
            gl.target.origin(),
            runtime_shared::primitives::graphics::FramebufferOrigin::BottomLeft,
            "the GL present blit flips V for a bottom-left framebuffer origin",
        );
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gpu-surface-gl-present"),
        });
        self.present.draw(&self.device, &mut encoder, &self.target_view, &fbo_view);
        self.queue.submit([encoder.finish()]);
    }
}

/// GL (GTK4): the adopted context must be current to DROP anything derived
/// from it — `new_external`'s adapter never makes it current for you, so
/// dropping the device/textures off a non-current context is UB. This runs
/// before the fields drop (Rust drops fields after `Drop::drop` returns). A
/// renderer state holding a `GpuSurface` should declare it as its FIRST field
/// so its own GPU objects drop after this has made the context current.
#[cfg(target_os = "linux")]
impl Drop for GpuSurface {
    fn drop(&mut self) {
        self.make_current();
    }
}

/// `request_device` with the adapter's OWN limits and `SHADER_F16` when the
/// adapter has it.
///
/// Limits: the default baseline asks for `max_inter_stage_shader_variables:
/// 16`, but iOS Metal (simulator and device) caps it at 15, so a default
/// request fails outright there. `adapter.limits()` is exactly what the GPU
/// provides — always grantable, never over-asks — and uniform across backends.
/// Each renderer validates its own minimums when building pipelines.
///
/// f16: requested whenever offered (harmless to pipelines that don't use it;
/// required by vello on Vulkan). On GL the adapter never lists it, so nothing
/// is requested there.
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

/// A renderer's per-surface state: owns the [`GpuSurface`] plus whatever the
/// renderer builds on its device. Exists exactly while the surface does.
pub trait SurfaceState: 'static {
    /// What `paint` produces each reactive run (a canvas `Scene`, a 3D scene).
    type Frame: 'static;
    fn gpu(&self) -> &GpuSurface;
    fn gpu_mut(&mut self) -> &mut GpuSurface;
    /// Draw `frame` into the frame target and present it. `true` iff a frame
    /// was presented (a `false` first frame is retried — see [`mount`]).
    fn render(&mut self, frame: &Self::Frame) -> bool;
    /// The surface was resized ([`GpuSurface::resize`] already ran): drop
    /// resources sized to the old drawable.
    fn resized(&mut self) {}
}

/// Create the `graphics` node for a GPU renderer and drive its lifecycle:
///
/// - **paint**: run inside a reactive effect (owned by the current component
///   scope), so any signal it reads re-paints and re-renders the view.
/// - **report**: told the LOGICAL size (physical / scale) on ready and every
///   resize, before the frame renders — what the painter reads as its size.
/// - **build**: turns a fresh [`GpuSurface`] into the renderer's state (called
///   on every `on_ready`, including after an `on_lost`). `None` leaves the view
///   blank and logs.
///
/// On resize the surface is reconfigured, [`SurfaceState::resized`] runs, and
/// the latest frame re-renders. On lost the state is dropped.
pub fn mount<H, S, P, R, B>(
    backend: &mut H,
    label: &'static str,
    req: Requirements,
    frame_alpha: FrameAlpha,
    paint: P,
    report: R,
    build: B,
) -> H::Node
where
    H: GraphicsOps,
    S: SurfaceState,
    P: Fn() -> S::Frame + 'static,
    R: Fn(f32, f32) + Clone + 'static,
    B: FnMut(GpuSurface) -> Option<S> + 'static,
{
    let report_logical = move |size: (u32, u32), scale: f32| {
        let s = if scale > 0.0 { scale } else { 1.0 };
        report(size.0 as f32 / s, size.1 as f32 / s);
    };
    let frame_cell: Rc<RefCell<Option<S::Frame>>> = Rc::new(RefCell::new(None));
    let state_cell: Rc<RefCell<Option<S>>> = Rc::new(RefCell::new(None));

    // Reactive repaint. On the first run the surface isn't ready yet; on_ready
    // does the first draw. Created here, in the mount walker, so the component
    // scope owns it.
    {
        let frame_cell = frame_cell.clone();
        let state_cell = state_cell.clone();
        runtime_world::effect(move || {
            let frame = paint();
            *frame_cell.borrow_mut() = Some(frame);
            if let Some(state) = state_cell.borrow_mut().as_mut() {
                if let Some(frame) = frame_cell.borrow().as_ref() {
                    state.render(frame);
                }
            }
        });
    }

    let on_ready = {
        let frame_cell = frame_cell.clone();
        let state_cell = state_cell.clone();
        let report_logical = report_logical.clone();
        let mut build = build;
        move |ev: OnReadyEvent| {
            let (size, scale) = (ev.size, ev.scale);
            report_logical(size, scale);
            // The lent GL target is cloned BEFORE `GpuSurface::new` consumes
            // `ev.target`, so the in-signal blit can be registered on it.
            #[cfg(target_os = "linux")]
            let gl_target = match &ev.target {
                GraphicsTarget::Gl(g) => Some(g.clone()),
                GraphicsTarget::RawWindow(_) => None,
            };
            let Some(gpu) = GpuSurface::new(ev.target, size, scale, req, frame_alpha, label) else {
                log::warn!("{label}: GPU surface bring-up failed — view stays blank");
                return;
            };
            let Some(mut state) = build(gpu) else {
                log::warn!("{label}: renderer build failed — view stays blank");
                return;
            };
            let presented = match frame_cell.borrow().as_ref() {
                Some(frame) => state.render(frame),
                None => true,
            };
            *state_cell.borrow_mut() = Some(state);
            #[cfg(target_os = "linux")]
            if let Some(gl_target) = gl_target {
                gl_target.on_render(Box::new({
                    let state_cell = state_cell.clone();
                    move || {
                        // `try_borrow_mut`: a present() emitted from within a
                        // render pass must not re-enter.
                        if let Ok(s) = state_cell.try_borrow() {
                            if let Some(s) = s.as_ref() {
                                s.gpu().blit_to_fbo();
                            }
                        }
                    }
                }));
            }
            // The first drawable often isn't acquirable on the deferred
            // on_ready tick (macOS CAMetalLayer): retry until it lands so the
            // view doesn't sit dark until the first reactive repaint.
            if !presented {
                retry_first_frame(frame_cell.clone(), state_cell.clone(), FIRST_FRAME_ATTEMPTS);
            }
        }
    };

    let on_resize = {
        let frame_cell = frame_cell.clone();
        let state_cell = state_cell.clone();
        move |ev: OnResizeEvent| {
            report_logical(ev.size, ev.scale);
            if let Some(state) = state_cell.borrow_mut().as_mut() {
                state.gpu_mut().resize(ev.size, ev.scale);
                state.resized();
                if let Some(frame) = frame_cell.borrow().as_ref() {
                    state.render(frame);
                }
            }
        }
    };

    let on_lost = {
        let state_cell = state_cell.clone();
        // Drop all GPU state derived from the lost surface; a fresh on_ready
        // follows if the surface returns.
        move || *state_cell.borrow_mut() = None
    };

    backend.create_graphics(
        Box::new(on_ready),
        Box::new(on_resize),
        Box::new(on_lost),
        &AccessibilityProps::default(),
    )
}

/// First-frame retries at the ~16 ms cadence below: about two seconds.
const FIRST_FRAME_ATTEMPTS: u32 = 120;
const FIRST_FRAME_RETRY_MS: i32 = 16;

/// Retry the first frame on a ~frame cadence until it presents (bounded).
/// `after_ms_detached` is reentrancy-safe and parked by the runtime.
fn retry_first_frame<S: SurfaceState>(
    frame_cell: Rc<RefCell<Option<S::Frame>>>,
    state_cell: Rc<RefCell<Option<S>>>,
    attempts_left: u32,
) {
    if attempts_left == 0 {
        return;
    }
    runtime_shared::scheduling::after_ms_detached(FIRST_FRAME_RETRY_MS, move || {
        let presented = match (state_cell.borrow_mut().as_mut(), frame_cell.borrow().as_ref()) {
            (Some(state), Some(frame)) => state.render(frame),
            _ => return, // surface lost / view dropped — stop retrying
        };
        if !presented {
            retry_first_frame(frame_cell, state_cell, attempts_left - 1);
        }
    });
}

// ============================================================================
// GL present path (Linux/GTK4)
// ============================================================================
//
// Every piece below is the proven `host-linux-desktop::linux` / `backend-linux`
// adoption, moved here from canvas-vello. The present itself is `PresentBlit`
// with `flip_y` (GTK composites the GLArea framebuffer bottom-up).

#[cfg(target_os = "linux")]
use runtime_shared::primitives::graphics::GlTarget;

#[cfg(target_os = "linux")]
struct GlPresent {
    target: GlTarget,
}

#[cfg(target_os = "linux")]
const GL_FRAMEBUFFER: u32 = 0x8D40;
#[cfg(target_os = "linux")]
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
#[cfg(target_os = "linux")]
const GL_FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE: u32 = 0x8CD0;
#[cfg(target_os = "linux")]
const GL_FRAMEBUFFER_ATTACHMENT_OBJECT_NAME: u32 = 0x8CD1;
#[cfg(target_os = "linux")]
const GL_TEXTURE: i32 = 0x1702;
#[cfg(target_os = "linux")]
const GL_RENDERBUFFER: i32 = 0x8D41;

/// GTK's own framebuffer attachment format: plain `Rgba8Unorm`, the same as the
/// frame target, so the blit stores bytes verbatim (no re-encode).
#[cfg(target_os = "linux")]
const FRAMEBUFFER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Adopt the lent GL context as a wgpu `(device, queue)` on wgpu's GL backend.
/// `None` if the driver can't satisfy `new_external` (GL below the GLES 3.0
/// floor) or `request_device` is rejected.
///
/// Limits are the adapter's own, not `downlevel_defaults()`: vello binds 5
/// storage buffers in one stage, over the downlevel floor's 4. No features are
/// requested (f16 is never listed on GL).
///
/// SAFETY: `new_external` requires the context be current for this call and
/// for every use/drop of what it returns; the caller ran `make_current()`.
#[cfg(target_os = "linux")]
fn adopt(gl: &GlTarget, label: &'static str) -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let exposed = unsafe {
        wgpu_hal::gles::Adapter::new_external(
            |sym| gl.get_proc_address(sym),
            wgpu_types::GlBackendOptions::default(),
        )
    }?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::GL,
        flags: wgpu::InstanceFlags::empty(),
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });
    // SAFETY: as above — the adapter is built from the current context.
    let adapter = unsafe { instance.create_adapter_from_hal(exposed) };
    let (device, queue) = pollster::block_on(request_device(&adapter, label)).ok()?;
    Some((adapter, device, queue))
}

/// Wrap the lent framebuffer's colour attachment as a wgpu texture.
#[cfg(target_os = "linux")]
fn wrap_framebuffer(
    device: &wgpu::Device,
    gl: &GlTarget,
    w: u32,
    h: u32,
) -> Option<wgpu::Texture> {
    let get: unsafe extern "C" fn(u32, u32, u32, *mut i32) =
        gl_fn(gl, "glGetFramebufferAttachmentParameteriv")?;
    let (mut kind, mut name) = (0i32, 0i32);
    // SAFETY: called with the context current and GTK's framebuffer bound
    // (`make_current` runs `gtk_gl_area_attach_buffers`).
    unsafe {
        get(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE, &mut kind);
        get(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_FRAMEBUFFER_ATTACHMENT_OBJECT_NAME, &mut name);
    }
    let name = std::num::NonZeroU32::new(u32::try_from(name).ok()?)?;

    let desc = wgpu::TextureDescriptor {
        label: Some("gtk-glarea-framebuffer"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FRAMEBUFFER_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    };
    let hal_desc = wgpu_hal::TextureDescriptor {
        label: desc.label,
        size: desc.size,
        mip_level_count: desc.mip_level_count,
        sample_count: desc.sample_count,
        dimension: desc.dimension,
        format: desc.format,
        usage: wgpu_types::TextureUses::COLOR_TARGET,
        memory_flags: wgpu_hal::MemoryFlags::empty(),
        view_formats: vec![],
    };
    // SAFETY: `name` is GTK's live colour attachment, described faithfully above.
    // The no-op `drop_callback` is load-bearing: it tells wgpu-hal the object is
    // BORROWED, so dropping this texture does not delete GTK's attachment.
    let hal_texture = unsafe {
        let dev = device.as_hal::<wgpu_hal::api::Gles>()?;
        match kind {
            GL_TEXTURE => dev.texture_from_raw(name, &hal_desc, Some(Box::new(|| {}))),
            GL_RENDERBUFFER => {
                dev.texture_from_raw_renderbuffer(name, &hal_desc, Some(Box::new(|| {})))
            }
            _ => return None,
        }
    };
    // SAFETY: `hal_texture` was just built from this device.
    Some(unsafe { device.create_texture_from_hal::<wgpu_hal::api::Gles>(hal_texture, &desc) })
}

#[cfg(target_os = "linux")]
fn gl_fn<T: Copy>(gl: &GlTarget, symbol: &str) -> Option<T> {
    let p = gl.get_proc_address(symbol);
    if p.is_null() {
        return None;
    }
    debug_assert_eq!(
        std::mem::size_of::<T>(),
        std::mem::size_of::<*const std::ffi::c_void>()
    );
    Some(unsafe { *(&p as *const *const std::ffi::c_void as *const T) })
}
