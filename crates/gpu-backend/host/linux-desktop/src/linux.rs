//! Linux-only implementation of [`crate::mount`]. Mirrors
//! `host-macos-desktop::macos` above the GPU boundary; the deltas are
//! all forced by GTK4 not having a per-widget native surface.
//!
//! # Three deltas from every swapchain host
//!
//! 1. **No surface.** The backend lends a live GL context
//!    ([`GlTarget`]) from a `GtkGLArea`. wgpu adopts it via
//!    `wgpu_hal::gles::Adapter::new_external` +
//!    `Instance::create_adapter_from_hal`. There is no
//!    `create_surface`, no `configure`, no `get_current_texture`, and
//!    nothing to reconfigure on resize — GTK owns the buffers.
//!
//! 2. **Context currency is the caller's job.** `new_external` builds
//!    an adapter that holds no EGL handle and never makes the context
//!    current for you; touching *anything* derived from it — including
//!    dropping it — is UB unless the context is current. So every entry
//!    point here starts with [`GlTarget::make_current`], and all GPU
//!    work is confined to a single synchronous span before returning to
//!    the GLib main loop.
//!
//! 3. **The frame is rendered offscreen and flip-blitted.** Two reasons
//!    it can't go straight into GTK's framebuffer:
//!
//!    - *Orientation.* `GlTarget::origin()` is `BottomLeft`. wgpu
//!      writes top-left-origin content and GDK composites the GL
//!      texture bottom-up, so a direct render lands upside down.
//!      (Measured, not assumed — `backend-linux`'s
//!      `gl_target_adoption` test pins both halves of that.)
//!    - *Colour.* `render-wgpu` expects an sRGB render target so hex
//!      colours land without manual gamma encoding, but GTK's
//!      framebuffer is plain `RGBA8`.
//!
//!    Both are solved by the same intermediate texture, at the cost of
//!    one full-frame blit. The target is `Rgba8UnormSrgb`, so the
//!    renderer's output is gamma-encoded on store exactly as it would be
//!    against a real sRGB swapchain. The blit then samples it (the
//!    hardware decodes sRGB → linear), **re-encodes to sRGB in the
//!    fragment shader**, and writes those bytes into GTK's plain
//!    `RGBA8` buffer, which stores them verbatim — so what GDK
//!    composites is gamma-encoded, matching every other backend.
//!
//!    The re-encode is done in WGSL rather than by sampling a linear
//!    *view* of the same texture (the obvious trick, and what
//!    `canvas-vello` does against a swapchain) because that needs
//!    `DownlevelFlags::VIEW_FORMATS`, which **wgpu's GL backend does not
//!    support** — `create_texture` rejects any `view_formats` outright.
//!    Doing the transfer function explicitly costs a few ALU ops per
//!    pixel and works on every driver.
//!
//!    The blit's vertex shader inverts V, which is where the flip
//!    happens — free, since the blit has to exist anyway.

use std::cell::RefCell;
use std::rc::Rc;

use render_api::DeviceProfile;
use render_wgpu::{Host, Painter, Renderer};
use runtime_shared::driver::{render_loop, RenderLoop};
use runtime_shared::primitives::graphics::{FramebufferOrigin, GlTarget};
use runtime_scene::Element;

// --- GL constants for the framebuffer-attachment query -------------------
const GL_FRAMEBUFFER: u32 = 0x8D40;
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
const GL_FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE: u32 = 0x8CD0;
const GL_FRAMEBUFFER_ATTACHMENT_OBJECT_NAME: u32 = 0x8CD1;
const GL_TEXTURE: i32 = 0x1702;
const GL_RENDERBUFFER: i32 = 0x8D41;

/// Format of GTK's own framebuffer attachment, and so of the blit's
/// output: plain `RGBA8`, storing whatever bytes the shader writes.
const FRAMEBUFFER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Format of the offscreen target the scene is rendered into. sRGB so
/// `render-wgpu` sees the same target it would against a real swapchain.
const TARGET_SRGB: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

#[derive(Debug)]
pub enum MountError {
    /// wgpu could not adopt the lent GL context. Means the driver
    /// couldn't satisfy `new_external` — e.g. a GL version below the
    /// GLES 3.0 floor wgpu's GLES backend needs.
    AdoptContext,
    /// `Adapter::request_device` rejected the limits we asked for.
    RequestDevice,
    /// The embedding app's world does not exist yet — this embed was
    /// mounted before `backend_linux::newcore::start` booted the app.
    /// Fail fast rather than realize into a second, orphaned world.
    NoHostWorld,
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MountError::AdoptContext => write!(
                f,
                "host-linux-desktop: wgpu could not adopt the GtkGLArea's GL context"
            ),
            MountError::RequestDevice => {
                write!(f, "host-linux-desktop: wgpu request_device failed")
            }
            MountError::NoHostWorld => write!(
                f,
                "host-linux-desktop: no host world yet \
                 (backend_linux::newcore::mounted_world() is None)"
            ),
        }
    }
}

impl std::error::Error for MountError {}

/// Live handle for one embedded preview. Drop it to release the wgpu
/// device / queue and cancel the render loop. `!Send + !Sync` — the
/// interior state is single-threaded, and a GL context is thread-bound
/// besides.
pub struct LinuxHostHandle {
    /// The realized embedded scene. Declared FIRST so it unmounts before
    /// the GPU state in `inner` goes away.
    _app: render_wgpu::newcore::NewCoreApp,
    inner: Rc<RefCell<HostInner>>,
    /// Declared LAST so the loop outlives `inner`'s Rc clones inside
    /// the per-frame closure.
    _render_loop: RenderLoop,
}

impl LinuxHostHandle {
    /// Resize the render target to a new physical-pixel size. Call from
    /// the `Graphics` primitive's `on_resize`.
    ///
    /// Unlike the swapchain hosts there is no surface to reconfigure —
    /// GTK reallocates its own framebuffer — so this only re-creates
    /// the offscreen target we render into.
    pub fn resize(&self, size: (u32, u32)) {
        let mut inner = self.inner.borrow_mut();
        if inner.size == size {
            return;
        }
        inner.size = (size.0.max(1), size.1.max(1));
        inner.gl.make_current();
        inner.target = Target::new(&inner.device, inner.size);
    }

    /// Drop the embedded app's reactive scope, keeping the GPU state.
    /// Pair with [`resume`](Self::resume) for `unmountOnBlur`.
    /// No-op. v2 removed `Host::unmount`, and there is no host-side
    /// visibility gate to re-implement it against — identical to
    /// `MacosHostHandle::pause` and `WebHostHandle::pause`. Kept so the
    /// `unmountOnBlur` call sites compile unchanged across hosts.
    pub fn pause(&self) {
        log::warn!("{}: pause() is a no-op (no host visibility gate)", env!("CARGO_PKG_NAME"));
    }

    /// Re-mount the embedded app from its cached `build_ui`. Idempotent.
    /// No-op — see [`LinuxHostHandle::pause`].
    pub fn resume(&self) {
        log::warn!("{}: resume() is a no-op (no host visibility gate)", env!("CARGO_PKG_NAME"));
    }

    /// True iff the embedded app is currently mounted.
    /// True while the handle is alive: it owns the embedded app for its
    /// whole lifetime now that mount/unmount is gone.
    pub fn is_running(&self) -> bool {
        true
    }
}

/// Mount the wgpu render backend on a `GtkGLArea`'s lent GL context.
/// Call from the `Graphics` primitive's `on_ready`, stash the handle so
/// `on_resize` / `on_lost` can resize or drop it.
pub async fn mount(
    gl: GlTarget,
    size: (u32, u32),
    profile: DeviceProfile,
    skin: Rc<dyn Painter>,
    build_ui: Rc<dyn Fn() -> Element + 'static>,
) -> Result<LinuxHostHandle, MountError> {
    let size = (size.0.max(1), size.1.max(1));

    // The app's world must exist BEFORE the async GPU init runs — fail
    // fast on a mis-sequenced boot rather than realizing into a second,
    // orphaned world.
    let world = backend_linux::newcore::mounted_world().ok_or(MountError::NoHostWorld)?;

    // 1. Adopt the lent context. Must be current for this and for every
    //    later use of what it returns (see the module header).
    gl.make_current();
    let (device, queue) = adopt(&gl).await?;

    // 2. Render-side stack. The renderer is built for the sRGB view of
    //    the offscreen target, NOT for GTK's framebuffer format — see
    //    the colour note in the module header.
    let session_scope = runtime_shared::session::push_scope();
    let renderer = Renderer::new(&device, &queue, TARGET_SRGB);
    let target = Target::new(&device, size);
    let blit = FlipBlit::new(&device);

    let mut host = Host::new(skin, profile.color_scheme);
    let logical = (profile.logical_size.0 as f32, profile.logical_size.1 as f32);
    host.set_viewport(logical.0, logical.1);
    // Realize into the EMBEDDING app's world so one flush driver commits
    // both trees — one thread, one world, one update stream. Mirrors
    // host-macos-desktop.
    let app = render_wgpu::newcore::start_in_world(
        host.backend().clone(),
        |_| {},
        {
            let build_ui = build_ui.clone();
            move || (&*build_ui)()
        },
        world,
    );

    // Linux doesn't fetch pending font URLs (embedded faces cover the
    // preview); log the count for diagnosis, same as macOS.
    let pending = host.take_pending_font_urls();
    if !pending.is_empty() {
        log::debug!(
            "host-linux-desktop: skipped fetch for {} pending font URL(s); \
             cosmic-text will fall back to embedded faces",
            pending.len()
        );
    }

    let inner = Rc::new(RefCell::new(HostInner {
        gl,
        device,
        queue,
        renderer,
        target,
        blit,
        host,
        logical,
        size,
        build_ui,
        presented_once: false,
        warned_no_framebuffer: false,
        _session_scope: session_scope,
    }));

    // 3. Per-frame loop. On GTK this rides the driver `host-gtk`
    //    installs (`scheduler::install_render_loop`); without it the
    //    handle is inert and nothing ever paints.
    // `eprintln!`, not `log::info!` — matching `host-macos-desktop`'s
    // first-frame diagnostic. This is the scriptable evidence that the
    // embedded preview actually came up, and it must not depend on
    // whether the embedding app installed a logger or how it filters.
    eprintln!(
        "[host-linux-desktop] mounted ({}x{} px, logical {}x{})",
        size.0, size.1, logical.0, logical.1,
    );
    // Register the in-signal flip-blit. `draw_frame` renders the scene into the
    // offscreen target out-of-band and schedules a pass via `present()`; GTK's
    // render signal then runs THIS callback (context current + FBO bound) to
    // blit into the framebuffer — the only place GtkGLArea composites what's
    // drawn (see `present_blit`). `try_borrow_mut` guards the (non-overlapping)
    // re-entrancy against the render loop's own `draw_frame` borrow.
    {
        let inner_for_blit = inner.clone();
        inner.borrow().gl.on_render(Box::new(move || {
            if let Ok(mut inner) = inner_for_blit.try_borrow_mut() {
                present_blit(&mut inner);
            }
        }));
    }

    let inner_for_frame = inner.clone();
    let render_loop_handle = render_loop(move |_elapsed| {
        let mut inner = inner_for_frame.borrow_mut();
        draw_frame(&mut inner);
    });

    Ok(LinuxHostHandle { _app: app, inner, _render_loop: render_loop_handle })
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

struct HostInner {
    gl: GlTarget,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    target: Target,
    blit: FlipBlit,
    host: Host,
    /// Logical viewport in CSS px from the `DeviceProfile`.
    logical: (f32, f32),
    /// Physical drawable size.
    size: (u32, u32),
    build_ui: Rc<dyn Fn() -> Element + 'static>,
    presented_once: bool,
    warned_no_framebuffer: bool,
    /// RAII guard for this host's `session::REGISTRY` scope. Declared
    /// LAST so it pops after the renderer / host / wgpu objects drop.
    _session_scope: runtime_shared::session::ScopeGuard,
}

/// The GL context must be current to drop anything derived from the
/// adapter (`new_external`'s safety contract). Nothing else in the
/// process guarantees that at drop time, so re-establish it here.
impl Drop for HostInner {
    fn drop(&mut self) {
        self.gl.make_current();
        runtime_shared::scheduling::set_frame_active(true);
    }
}

/// Offscreen render target the scene is drawn into before being
/// flip-blitted into GTK's framebuffer.
///
/// Deliberately ONE view and no `view_formats`: wgpu's GL backend
/// doesn't advertise `DownlevelFlags::VIEW_FORMATS`, so asking for a
/// second view format fails `create_texture` outright. The colour
/// conversion the second view would have done happens in the blit
/// shader instead.
struct Target {
    view: wgpu::TextureView,
}

impl Target {
    fn new(device: &wgpu::Device, (w, h): (u32, u32)) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("host-linux-desktop-target"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_SRGB,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Self { view: texture.create_view(&wgpu::TextureViewDescriptor::default()) }
    }
}

/// Fullscreen blit that inverts V on the way out.
struct FlipBlit {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
}

/// A fullscreen triangle whose UVs are derived from clip space with the
/// V axis **inverted**: the usual mapping is `v = (1 - y) / 2`, and
/// using `(y + 1) / 2` instead is the entire flip. It costs nothing —
/// the blit exists anyway to bridge the colour-space mismatch.
const FLIP_BLIT_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) idx: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );
    let xy = corners[idx];
    var out: VsOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = vec2<f32>((xy.x + 1.0) * 0.5, (xy.y + 1.0) * 0.5);
    return out;
}

// Sampling an `Rgba8UnormSrgb` texture DECODES to linear, but the
// destination is a plain `RGBA8` buffer that stores bytes verbatim and
// GDK reads as gamma-encoded. So put the encoding back, matching the
// IEC 61966-2-1 transfer function the hardware would have applied.
fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(src, samp, in.uv);
    return vec4<f32>(linear_to_srgb(c.rgb), c.a);
}
"#;

impl FlipBlit {
    fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("host-linux-desktop-flip-blit"),
            source: wgpu::ShaderSource::Wgsl(FLIP_BLIT_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("host-linux-desktop-flip-blit-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("host-linux-desktop-flip-blit-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                // GTK's framebuffer is plain RGBA8; writing the
                // already-sRGB-encoded bytes through a linear target is
                // what keeps colours matching the other backends.
                targets: &[Some(FRAMEBUFFER_FORMAT.into())],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("host-linux-desktop-flip-blit-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Self { pipeline, sampler, layout }
    }

    fn draw(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
    ) {
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("host-linux-desktop-flip-blit-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Adopt the lent GL context as a wgpu device + queue.
async fn adopt(gl: &GlTarget) -> Result<(wgpu::Device, wgpu::Queue), MountError> {
    // SAFETY: `new_external`'s contract is that the GL context is
    // current for this call and for every use/drop of what it returns.
    // The caller has just run `make_current()`; `draw_frame`, `resize`
    // and `HostInner::drop` each re-establish it before touching
    // anything derived from this adapter.
    let exposed = unsafe {
        wgpu_hal::gles::Adapter::new_external(
            |sym| gl.get_proc_address(sym),
            wgpu_types::GlBackendOptions::default(),
        )
    }
    .ok_or(MountError::AdoptContext)?;

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::GL,
        flags: wgpu::InstanceFlags::empty(),
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });
    // SAFETY: as above — the adapter is built from the current context.
    let adapter = unsafe { instance.create_adapter_from_hal(exposed) };
    let adapter_limits = adapter.limits();
    // GL reports lower caps than Metal/Vulkan, so clamping the
    // downlevel baseline to what this adapter actually advertises is
    // load-bearing here rather than the no-op it is on desktop Metal.
    adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("host-linux-desktop-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults()
                .using_resolution(adapter_limits),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(|_| MountError::RequestDevice)
}

/// Wrap the lent framebuffer's colour attachment as a wgpu texture.
///
/// Re-queried every frame rather than cached: GTK reallocates the
/// `GtkGLArea`'s buffers across resizes and re-realizes, and rendering
/// into a stale attachment draws into freed GPU memory.
fn wrap_framebuffer(inner: &HostInner) -> Option<wgpu::Texture> {
    let get: unsafe extern "C" fn(u32, u32, u32, *mut i32) =
        gl_fn(&inner.gl, "glGetFramebufferAttachmentParameteriv")?;
    let (mut kind, mut name) = (0i32, 0i32);
    // SAFETY: called with the context current and GTK's framebuffer
    // bound (`make_current` runs `gtk_gl_area_attach_buffers`).
    unsafe {
        get(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE, &mut kind);
        get(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_FRAMEBUFFER_ATTACHMENT_OBJECT_NAME, &mut name);
    }
    let name = std::num::NonZeroU32::new(u32::try_from(name).ok()?)?;

    let (w, h) = inner.size;
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
    // SAFETY: `name` is GTK's live colour attachment, described
    // faithfully above. The no-op `drop_callback` is load-bearing: it
    // tells wgpu-hal the object is BORROWED, so dropping this texture
    // does not delete GTK's attachment out from under it.
    let hal_texture = unsafe {
        let dev = inner.device.as_hal::<wgpu_hal::api::Gles>()?;
        match kind {
            GL_TEXTURE => dev.texture_from_raw(name, &hal_desc, Some(Box::new(|| {}))),
            GL_RENDERBUFFER => {
                dev.texture_from_raw_renderbuffer(name, &hal_desc, Some(Box::new(|| {})))
            }
            _ => return None,
        }
    };
    // SAFETY: `hal_texture` was just built from this device.
    Some(unsafe {
        inner
            .device
            .create_texture_from_hal::<wgpu_hal::api::Gles>(hal_texture, &desc)
    })
}

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

fn draw_frame(inner: &mut HostInner) {
    // The scene render below is only legal with the context current and GTK's
    // framebuffer bound; this establishes both, and must be re-run each frame
    // because GTK may have changed either since the last one (resize, or
    // another GLArea rendering in between).
    inner.gl.make_current();
    runtime_shared::scheduling::set_frame_active(true);

    // Scene → offscreen (sRGB view). The flip-blit into GTK's framebuffer does
    // NOT happen here — it's deferred to `present_blit`, run INSIDE GTK's
    // `render` signal (registered via `GlTarget::on_render` at mount), because
    // GtkGLArea only composites what is drawn during that signal. Blitting here
    // (out-of-band) left the widget showing the pre-adoption clear colour — the
    // "GPU canvas stuck on the dark clear colour" bug.
    inner.renderer.render(
        &inner.host,
        &inner.device,
        &inner.queue,
        &inner.target.view,
        inner.logical,
        (0.0, 0.0, inner.size.0 as f32, inner.size.1 as f32),
    );

    // Schedule the render pass that runs `present_blit` (the FBO flip-blit).
    // GTK re-composites the GLArea on its next frame; the pass fires with the
    // context current + FBO bound, the only place the blit is composited.
    inner.gl.present();

    // Advance per-frame state (animations, spinners, momentum).
    let _ = inner.host.tick();
}

/// Flip-blit the offscreen `target` into GTK's framebuffer, **from inside GTK's
/// `render` signal**. Registered via [`GlTarget::on_render`] at mount; GTK runs
/// it with the context current and the area's FBO bound.
///
/// # The invariant this exists to honour (CLAUDE.md §5)
///
/// GtkGLArea composites ONLY what is drawn during its own render pass.
/// `draw_frame` renders the scene into the offscreen `target` out-of-band (the
/// driver's render loop) and calls `present()` (queue_render) to schedule a
/// pass; GTK then emits `render` and runs this — the one place the blit's
/// result is guaranteed on-screen. Blitting in `draw_frame` left the widget
/// stuck on the pre-adoption clear colour (the bug this fixes).
///
/// The sRGB re-encode stays in the blit shader (`self.target` is
/// `Rgba8UnormSrgb`, GTK's FBO plain `Rgba8Unorm`) — only WHERE the blit runs
/// changed, not what it does.
fn present_blit(inner: &mut HostInner) {
    // Defensive: GTK has already made the context current + bound the FBO
    // before emitting `render`, but re-establishing both is cheap and re-binds
    // exactly what `wrap_framebuffer` reads below.
    inner.gl.make_current();

    let Some(fbo_tex) = wrap_framebuffer(inner) else {
        // Once only: this fires every frame if it fires at all, and a silent
        // early return here is exactly the "mounts cleanly, renders nothing"
        // failure that is hardest to diagnose.
        if !inner.warned_no_framebuffer {
            inner.warned_no_framebuffer = true;
            eprintln!(
                "[host-linux-desktop] could not wrap the GtkGLArea framebuffer \
                 (fbo={}) — nothing will render",
                inner.gl.framebuffer(),
            );
        }
        return;
    };
    let fbo_view = fbo_tex.create_view(&wgpu::TextureViewDescriptor::default());

    // Offscreen → GTK's framebuffer, flipping V. Asserted rather than assumed:
    // if a backend ever reports a top-left GL framebuffer this blit would be
    // wrong, so it is only correct to flip while the target says `BottomLeft`.
    debug_assert_eq!(
        inner.gl.origin(),
        FramebufferOrigin::BottomLeft,
        "flip blit assumes a bottom-left framebuffer origin",
    );
    let mut encoder = inner
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("host-linux-desktop-present"),
        });
    inner
        .blit
        .draw(&inner.device, &mut encoder, &inner.target.view, &fbo_view);
    inner.queue.submit([encoder.finish()]);

    if !inner.presented_once {
        inner.presented_once = true;
        eprintln!(
            "[host-linux-desktop] first frame presented ({}x{} px, logical {}x{})",
            inner.size.0, inner.size.1, inner.logical.0, inner.logical.1,
        );
    }
}
