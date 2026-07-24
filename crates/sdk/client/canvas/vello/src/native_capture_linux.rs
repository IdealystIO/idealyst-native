//! Linux zero-copy capture target for canvas-vello — the dma-buf analog of the
//! macOS IOSurface ring in [`native_capture`](super::native_capture).
//!
//! A small ring of **linear** dma-buf buffers, each allocated through **GBM**
//! (`GBM_BO_USE_LINEAR`), imported as a GL texture (`EGL_LINUX_DMA_BUF_EXT` +
//! `glEGLImageTargetTexture2DOES`), and wrapped back into a wgpu texture
//! (`create_texture_from_hal`). Each render blits the vello target into the next
//! ring texture and publishes that buffer's [`DmaBufFrame`] descriptor to the
//! stream's native source. A GPU-rendered frame stays a GPU handle through the
//! whole stream — no CPU read-back in canvas-vello. A CPU consumer (the software
//! VP8 encoder) downloads at ITS boundary (GStreamer `gldownload`); a hardware
//! (VAAPI) encoder consumes the dma-buf with zero download.
//!
//! # Why LINEAR, and why not the wgpu render texture directly
//!
//! The obvious approach — export the wgpu `RENDER_ATTACHMENT` texture via
//! `EGL_MESA_image_dma_buf_export` — is what the first cut shipped, and it produced
//! **corrupt recordings**. A wgpu/Mesa render target is GPU-*tiled* and exports with
//! an *implicit* modifier (`DRM_FORMAT_MOD_INVALID`). An implicit modifier CANNOT be
//! expressed in GStreamer `memory:DMABuf`/`DMA_DRM` caps — a bare fourcc there means
//! `DRM_FORMAT_MOD_LINEAR` — so `glupload` read the tiled bytes as linear and
//! scrambled every frame. There is no way to say "tiled, layout unknown" to a
//! consumer. The fix is to make the buffer genuinely linear: GBM allocates a LINEAR
//! bo, we render into it, and it is published with an honest `LINEAR` modifier. The
//! pixel-correctness of the whole producer chain (GBM linear → EGLImage import →
//! wgpu render → CPU-visible linear bytes) is proven by
//! [`tests::gpu_render_into_linear_dmabuf_reads_back_pixel_correct`].
//!
//! # Why the ring texture must be a real GL texture
//!
//! wgpu-hal's GLES backend only wraps a real `GL_TEXTURE_2D` (not a renderbuffer).
//! The imported dma-buf IS a `GL_TEXTURE_2D` (from `glEGLImageTargetTexture2DOES`),
//! which we hand to `create_texture_from_hal` with `RENDER_ATTACHMENT |
//! TEXTURE_BINDING` so the blitter can draw into it.
//!
//! # fd / buffer lifetime
//!
//! Each ring [`PoolItem`] owns its GBM bo and an [`OwnedFd`] (from `gbm_bo_get_fd`)
//! for its whole lifetime. The published [`DmaBufFrame`] carries a **borrowed**
//! `RawFd` into that owned fd; the consumer `dup(2)`s it when it wraps the fd into a
//! `GstMemory`. The dma-buf fd holds an independent kernel reference to the buffer,
//! so the EGLImage is destroyed immediately after binding (the GL texture keeps its
//! own reference on Mesa — verified by the round-trip test rendering AFTER destroy).
//!
//! # Why a ring (not one texture)
//!
//! The consumer (GStreamer `appsrc`) reads asynchronously on its own thread.
//! Rendering the next frame into a DIFFERENT ring texture means the canvas never
//! overwrites the buffer the encoder is still reading. At ~60fps the blit completes
//! microseconds after submit — long before the texture is reused `POOL` frames later
//! — so the cadence is the sync (no explicit fence), as on macOS.

use media_stream::{DmaBufFrame, FrameWriter};
use std::ffi::{c_void, CString};
use std::os::fd::{FromRawFd, OwnedFd};
use std::rc::Rc;
use std::sync::OnceLock;

/// Textures in the ring. 3 = standard triple-buffering: enough that the encoder is
/// never reading the texture the canvas is rendering into.
const POOL: usize = 3;

// GL enums (stable constants) for an Rgba8Unorm imported texture.
const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_RGBA8: u32 = 0x8058;
const GL_RGBA: u32 = 0x1908;
const GL_UNSIGNED_BYTE: u32 = 0x1401;

/// DRM fourcc `AB24` = `DRM_FORMAT_ABGR8888` → bytes R,G,B,A in memory, which is
/// exactly wgpu `Rgba8Unorm`. So the blit into the ring texture is a straight copy.
const DRM_FORMAT_ABGR8888: u32 = u32::from_le_bytes([b'A', b'B', b'2', b'4']);
/// `DRM_FORMAT_MOD_LINEAR` — the honest, consumer-representable modifier.
const DRM_FORMAT_MOD_LINEAR: u64 = 0;

struct PoolItem {
    /// Keeps the GBM library + device alive; used to destroy `bo` on drop.
    gbm: Rc<Gbm>,
    /// The GBM buffer object backing this ring slot (destroyed on drop).
    bo: *mut c_void,
    /// Owns the exported dma-buf fd for the item's lifetime (closed on drop). The
    /// published descriptor borrows it.
    _fd: OwnedFd,
    /// wgpu view of the imported ring texture (the blit's render target).
    view: wgpu::TextureView,
    _texture: wgpu::Texture,
    frame: DmaBufFrame,
}

impl Drop for PoolItem {
    fn drop(&mut self) {
        // gbm_bo_destroy is a CPU-side buffer-management call (no GL context needed).
        // The dma-buf fd (OwnedFd) is closed by its own Drop after this.
        unsafe { (self.gbm.bo_destroy)(self.bo) };
    }
}

/// The canvas's Linux native-capture ring. Lazily (re)built to match the drawable
/// size; idle (empty pool) until a recorder taps the stream.
pub(crate) struct NativeCapture {
    writer: FrameWriter,
    pool: Vec<PoolItem>,
    next: usize,
    size: (u32, u32),
    /// The GBM device (opened on the DRM render node), shared by every ring item.
    gbm: Option<Rc<Gbm>>,
    /// `Rgba8Unorm`→`Rgba8Unorm` straight-copy blitter (built lazily).
    blitter: Option<wgpu::util::TextureBlitter>,
}

/// Whether the dma-buf zero-copy capture path is enabled. **ON by default** now that
/// the linear-export path is proven pixel-correct; set `IDEALYST_CANVAS_DMABUF=0`
/// (or `false`) to force the CPU read-back path instead. Read once, cached.
fn dmabuf_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("IDEALYST_CANVAS_DMABUF")
            .map(|v| !(v == "0" || v == "false"))
            .unwrap_or(true)
    })
}

impl NativeCapture {
    pub(crate) fn new(writer: FrameWriter) -> Self {
        Self { writer, pool: Vec::new(), next: 0, size: (0, 0), gbm: None, blitter: None }
    }

    /// True only while a recorder holds a `NativeTap` on the stream's dma-buf
    /// source — gates all the GPU export work so an un-recorded canvas pays nothing.
    /// Also gated by [`dmabuf_enabled`] (opt out with `IDEALYST_CANVAS_DMABUF=0`).
    pub(crate) fn wants(&self) -> bool {
        dmabuf_enabled() && self.writer.wants_native()
    }

    /// Blit `src_view` (the vello `Rgba8Unorm` target) into the next ring texture,
    /// recording the copy into `encoder` (submitted with the frame). Returns the
    /// ring index to [`publish`](Self::publish) AFTER the submit, or `None` if the
    /// pool couldn't be built (export unsupported → caller stays on CPU read-back).
    pub(crate) fn blit_into(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        w: u32,
        h: u32,
    ) -> Option<usize> {
        self.ensure_pool(device, w, h);
        let (Some(blitter), false) = (self.blitter.as_ref(), self.pool.is_empty()) else {
            return None;
        };
        let idx = self.next;
        blitter.copy(device, encoder, src_view, &self.pool[idx].view);
        self.next = (self.next + 1) % self.pool.len();
        Some(idx)
    }

    /// Publish the ring texture at `idx`'s dma-buf descriptor to the stream's native
    /// source. Call after the GPU submit so the blit is in flight.
    pub(crate) fn publish(&self, idx: usize) {
        self.writer.publish_dmabuf(self.pool[idx].frame);
    }

    /// (Re)build the ring when the drawable size changes (or on first use). Runs with
    /// the GL context current (the caller's `render()` made it current, or — in a
    /// headless wgpu-GL device — the adapter-context lock makes it current). A build
    /// failure (no GBM, no EGL import, wgpu wrap rejected) leaves the pool empty, so
    /// `blit_into` returns `None` and the canvas falls back to CPU read-back.
    fn ensure_pool(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        if !self.pool.is_empty() && self.size == (w, h) {
            return;
        }
        self.pool.clear();
        self.next = 0;
        self.size = (w, h);
        if w == 0 || h == 0 {
            return;
        }
        if self.blitter.is_none() {
            self.blitter =
                Some(wgpu::util::TextureBlitter::new(device, wgpu::TextureFormat::Rgba8Unorm));
        }
        if self.gbm.is_none() {
            self.gbm = Gbm::open().map(Rc::new);
        }
        let (Some(gbm), Some(egl)) = (self.gbm.clone(), egl::Egl::get()) else {
            // No GBM device or no EGL import entry points — stay on CPU read-back.
            return;
        };
        for _ in 0..POOL {
            let Some(item) = make_pool_item(device, &gbm, egl, w, h) else {
                self.pool.clear();
                return;
            };
            self.pool.push(item);
        }
    }
}

/// Allocate one LINEAR GBM buffer, import it as a GL texture, and wrap it as a wgpu
/// texture. `None` (→ CPU fallback) on any GBM/EGL/wgpu failure.
fn make_pool_item(
    device: &wgpu::Device,
    gbm: &Rc<Gbm>,
    egl: &egl::Egl,
    w: u32,
    h: u32,
) -> Option<PoolItem> {
    // 1) LINEAR GBM bo → dma-buf fd + stride. LINEAR is the whole point (see docs).
    let bo = unsafe {
        (gbm.bo_create)(
            gbm.device,
            w,
            h,
            DRM_FORMAT_ABGR8888,
            gbm::USE_RENDERING | gbm::USE_LINEAR,
        )
    };
    if bo.is_null() {
        return None;
    }
    let stride = unsafe { (gbm.bo_get_stride)(bo) };
    let raw_fd = unsafe { (gbm.bo_get_fd)(bo) };
    if raw_fd < 0 {
        unsafe { (gbm.bo_destroy)(bo) };
        return None;
    }
    // Own the fd now so it is closed on any early return below.
    let owned = unsafe { OwnedFd::from_raw_fd(raw_fd) };

    // 2) Import as a GL texture INSIDE the adapter-context lock (makes the EGL
    //    context current on a wgpu-owned device; a no-op on an adopted/`new_external`
    //    device where GTK already made it current). Split BEFORE the wgpu call.
    let gl_name = {
        use glow::HasContext;
        let hal = unsafe { device.as_hal::<wgpu_hal::api::Gles>() }?;
        let gl = hal.context().lock();
        let name = unsafe {
            let dpy = (egl.get_current_display)();
            if dpy.is_null() {
                None
            } else {
                let image = egl.import_linear(dpy, raw_fd, w, h, DRM_FORMAT_ABGR8888, stride);
                if image.is_null() {
                    None
                } else {
                    let tex = gl.create_texture().ok();
                    tex.map(|tex| {
                        gl.bind_texture(GL_TEXTURE_2D, Some(tex));
                        (egl.image_target_texture_2d)(GL_TEXTURE_2D, image);
                        gl.bind_texture(GL_TEXTURE_2D, None);
                        // The GL texture holds its own reference to the buffer on
                        // Mesa; the fd keeps the kernel buffer alive independently.
                        (egl.destroy_image)(dpy, image);
                        tex.0.get()
                    })
                }
            }
        };
        // Lock drops here → EGL context made not-current on a wgpu-owned device.
        name
    };
    let gl_name = match gl_name {
        Some(n) => n,
        None => {
            unsafe { (gbm.bo_destroy)(bo) };
            drop(owned);
            return None;
        }
    };

    // 3) Wrap the imported GL texture as a wgpu texture (blit target). The lock must
    //    be free here: create_texture_from_hal takes its own glow lock internally.
    let hal_tex = wgpu_hal::gles::Texture {
        inner: wgpu_hal::gles::TextureInner::Texture {
            raw: glow::NativeTexture(std::num::NonZeroU32::new(gl_name)?),
            target: GL_TEXTURE_2D,
        },
        mip_level_count: 1,
        array_layer_count: 1,
        format: wgpu::TextureFormat::Rgba8Unorm,
        format_desc: wgpu_hal::gles::TextureFormatDesc {
            internal: GL_RGBA8,
            external: GL_RGBA,
            data_type: GL_UNSIGNED_BYTE,
        },
        copy_size: wgpu_hal::CopyExtent { width: w, height: h, depth: 1 },
        // We own the GL texture's lifetime (destroyed with the bo/fd); no drop guard.
        drop_guard: None,
    };
    let desc = wgpu::TextureDescriptor {
        label: Some("canvas-vello-dmabuf-linear-ring"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    };
    let texture = unsafe { device.create_texture_from_hal::<wgpu_hal::api::Gles>(hal_tex, &desc) };
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let frame = DmaBufFrame {
        fd: raw_fd, // borrows `owned`; valid while this PoolItem lives
        width: w,
        height: h,
        fourcc: DRM_FORMAT_ABGR8888,
        stride: stride as i32,
        offset: 0,
        modifier: DRM_FORMAT_MOD_LINEAR,
    };
    Some(PoolItem { gbm: gbm.clone(), bo, _fd: owned, view, _texture: texture, frame })
}

// ============================================================================
// GBM — dlopen `libgbm.so.1` and open the DRM render node. Runtime-resolved (no
// build-time link), matching the module's EGL loader philosophy and keeping the
// dependency off the crate's default link set.
// ============================================================================

mod gbm {
    pub const USE_RENDERING: u32 = 1 << 2;
    pub const USE_LINEAR: u32 = 1 << 4;
}

/// The DRM render nodes to try, in order. renderD128 is the first/primary GPU on a
/// single-GPU box; the fallback covers a second node.
const RENDER_NODES: [&std::ffi::CStr; 2] = [c"/dev/dri/renderD128", c"/dev/dri/renderD129"];

/// Resolved GBM entry points + an open device on a DRM render node.
struct Gbm {
    device: *mut c_void,
    /// The open render node. Held for the device's lifetime (the GBM device uses it);
    /// closed by `OwnedFd`'s drop AFTER `Drop for Gbm` runs `gbm_device_destroy`.
    _drm_fd: OwnedFd,
    bo_create: unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32) -> *mut c_void,
    bo_get_fd: unsafe extern "C" fn(*mut c_void) -> i32,
    bo_get_stride: unsafe extern "C" fn(*mut c_void) -> u32,
    bo_destroy: unsafe extern "C" fn(*mut c_void),
    device_destroy: unsafe extern "C" fn(*mut c_void),
}

impl Gbm {
    /// dlopen libgbm, resolve the entry points, open a render node, create a device.
    /// `None` if libgbm is absent, a symbol is missing, or no render node opens.
    fn open() -> Option<Gbm> {
        unsafe {
            let mut h = libc::dlopen(c"libgbm.so.1".as_ptr(), libc::RTLD_LAZY);
            if h.is_null() {
                h = libc::dlopen(c"libgbm.so".as_ptr(), libc::RTLD_LAZY);
            }
            if h.is_null() {
                return None;
            }
            let create_device: unsafe extern "C" fn(i32) -> *mut c_void =
                dlsym_fn(h, "gbm_create_device")?;
            let bo_create = dlsym_fn(h, "gbm_bo_create")?;
            let bo_get_fd = dlsym_fn(h, "gbm_bo_get_fd")?;
            let bo_get_stride = dlsym_fn(h, "gbm_bo_get_stride")?;
            let bo_destroy = dlsym_fn(h, "gbm_bo_destroy")?;
            let device_destroy = dlsym_fn(h, "gbm_device_destroy")?;

            for node in RENDER_NODES {
                let fd = libc::open(node.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
                if fd < 0 {
                    continue;
                }
                let drm_fd = OwnedFd::from_raw_fd(fd);
                let device = create_device(fd);
                if device.is_null() {
                    // drm_fd drops → closes fd; try the next node.
                    continue;
                }
                return Some(Gbm {
                    device,
                    _drm_fd: drm_fd,
                    bo_create,
                    bo_get_fd,
                    bo_get_stride,
                    bo_destroy,
                    device_destroy,
                });
            }
            None
        }
    }
}

impl Drop for Gbm {
    fn drop(&mut self) {
        // Destroy the device before the render-node fd (OwnedFd) closes.
        unsafe { (self.device_destroy)(self.device) };
    }
}

// Raw pointers into libgbm are process-global once resolved; the device is only
// touched on the render thread.
unsafe impl Send for Gbm {}
unsafe impl Sync for Gbm {}

/// `dlsym` a symbol and transmute to fn-pointer type `T`. `None` if unresolved.
unsafe fn dlsym_fn<T: Copy>(handle: *mut c_void, name: &str) -> Option<T> {
    let c = CString::new(name).ok()?;
    let p = unsafe { libc::dlsym(handle, c.as_ptr()) };
    if p.is_null() {
        return None;
    }
    Some(unsafe { *(&p as *const *mut c_void as *const T) })
}

// ============================================================================
// EGL dma-buf IMPORT — resolved through `libEGL`'s `eglGetProcAddress`. `libEGL` is
// always present where a GL context exists; `eglGetProcAddress` returns
// context-independent extension pointers, so a freshly-`dlopen`'d libEGL resolves
// the same functions the adopted (GTK/epoxy) or headless (wgpu) context uses.
// ============================================================================

mod egl {
    use super::c_void;
    use std::ffi::CString;
    use std::sync::OnceLock;

    type EglDisplay = *mut c_void;
    type EglContext = *mut c_void;
    type EglImage = *mut c_void;
    type EglClientBuffer = *mut c_void;

    const EGL_NONE: i32 = 0x3038;
    const EGL_WIDTH: i32 = 0x3057;
    const EGL_HEIGHT: i32 = 0x3056;
    const EGL_LINUX_DMA_BUF_EXT: u32 = 0x3270;
    const EGL_LINUX_DRM_FOURCC_EXT: i32 = 0x3271;
    const EGL_DMA_BUF_PLANE0_FD_EXT: i32 = 0x3272;
    const EGL_DMA_BUF_PLANE0_OFFSET_EXT: i32 = 0x3273;
    const EGL_DMA_BUF_PLANE0_PITCH_EXT: i32 = 0x3274;
    const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: i32 = 0x3443;
    const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: i32 = 0x3444;
    const EGL_NO_CONTEXT: EglContext = std::ptr::null_mut();

    type FnGetProcAddress = unsafe extern "C" fn(*const std::ffi::c_char) -> *const c_void;
    type FnGetCurrentDisplay = unsafe extern "C" fn() -> EglDisplay;
    type FnCreateImageKHR = unsafe extern "C" fn(
        EglDisplay,
        EglContext,
        u32,
        EglClientBuffer,
        *const i32,
    ) -> EglImage;
    type FnDestroyImageKHR = unsafe extern "C" fn(EglDisplay, EglImage) -> u32;
    type FnImageTargetTexture2DOES = unsafe extern "C" fn(u32, EglImage);

    /// Resolved EGL dma-buf import entry points (process-global; built once).
    pub(super) struct Egl {
        pub(super) get_current_display: FnGetCurrentDisplay,
        create_image: FnCreateImageKHR,
        pub(super) destroy_image: FnDestroyImageKHR,
        pub(super) image_target_texture_2d: FnImageTargetTexture2DOES,
    }

    // The resolved fn pointers are process-global; only called on the render thread
    // with the GL context current.
    unsafe impl Send for Egl {}
    unsafe impl Sync for Egl {}

    static EGL: OnceLock<Option<Egl>> = OnceLock::new();

    impl Egl {
        /// Resolve the import entry points once, or `None` if unavailable.
        pub(super) fn get() -> Option<&'static Egl> {
            EGL.get_or_init(Egl::resolve).as_ref()
        }

        fn resolve() -> Option<Egl> {
            let get_proc = egl_get_proc_address()?;
            unsafe {
                Some(Egl {
                    get_current_display: transmute_fn(get_proc, "eglGetCurrentDisplay")?,
                    create_image: transmute_fn(get_proc, "eglCreateImageKHR")?,
                    destroy_image: transmute_fn(get_proc, "eglDestroyImageKHR")?,
                    image_target_texture_2d: transmute_fn(
                        get_proc,
                        "glEGLImageTargetTexture2DOES",
                    )?,
                })
            }
        }

        /// Import a single-plane LINEAR dma-buf as an `EGLImage`. `EGL_NO_IMAGE`
        /// (null) on failure.
        ///
        /// # Safety
        /// `dpy` must be a valid current EGL display and `fd` a live dma-buf fd
        /// describing a `fourcc`/`stride` LINEAR buffer of `w × h`.
        pub(super) unsafe fn import_linear(
            &self,
            dpy: EglDisplay,
            fd: i32,
            w: u32,
            h: u32,
            fourcc: u32,
            stride: u32,
        ) -> EglImage {
            let attrs: [i32; 17] = [
                EGL_WIDTH, w as i32,
                EGL_HEIGHT, h as i32,
                EGL_LINUX_DRM_FOURCC_EXT, fourcc as i32,
                EGL_DMA_BUF_PLANE0_FD_EXT, fd,
                EGL_DMA_BUF_PLANE0_OFFSET_EXT, 0,
                EGL_DMA_BUF_PLANE0_PITCH_EXT, stride as i32,
                EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, 0, // explicit DRM_FORMAT_MOD_LINEAR
                EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, 0,
                EGL_NONE,
            ];
            unsafe {
                (self.create_image)(
                    dpy,
                    EGL_NO_CONTEXT,
                    EGL_LINUX_DMA_BUF_EXT,
                    std::ptr::null_mut(),
                    attrs.as_ptr(),
                )
            }
        }
    }

    /// Resolve `symbol` through `get_proc` and transmute to fn-pointer type `T`.
    unsafe fn transmute_fn<T: Copy>(get_proc: FnGetProcAddress, symbol: &str) -> Option<T> {
        let cname = CString::new(symbol).ok()?;
        let p = unsafe { get_proc(cname.as_ptr()) };
        if p.is_null() {
            return None;
        }
        Some(unsafe { *(&p as *const *const c_void as *const T) })
    }

    /// `dlopen` libEGL (refcount bump if already mapped) and read `eglGetProcAddress`.
    fn egl_get_proc_address() -> Option<FnGetProcAddress> {
        static PTR: OnceLock<Option<usize>> = OnceLock::new();
        let raw = (*PTR.get_or_init(|| unsafe {
            let mut handle = libc::dlopen(c"libEGL.so.1".as_ptr(), libc::RTLD_LAZY);
            if handle.is_null() {
                handle = libc::dlopen(c"libEGL.so".as_ptr(), libc::RTLD_LAZY);
            }
            if handle.is_null() {
                return None;
            }
            let addr = libc::dlsym(handle, c"eglGetProcAddress".as_ptr());
            if addr.is_null() {
                return None;
            }
            Some(*(&addr as *const *mut c_void as *const usize))
        }))?;
        Some(unsafe { std::mem::transmute::<usize, FnGetProcAddress>(raw) })
    }
}

// ============================================================================
// LayerCompositor: composite a stack of `TextureLayer`s (live camera/screen
// `MediaStream`s, static images) INTO the canvas target — the same behavior as
// the macOS compositor (`native_capture.rs`), so the camera appears on-screen
// AND in the recording (CLAUDE.md §7: uniform output across backends). The one
// difference from macOS is the source: Apple imports the stream's zero-copy
// IOSurface as a Metal texture; on Linux the camera has no native GPU surface,
// so we upload its latest CPU `RGBA8` frame (`MediaStream::latest`) to a texture,
// re-uploading only when the frame `generation()` changes. Everything else — the
// blit shader, fit/rounded/border/opacity math, the draw — mirrors macOS.
//
// The WGSL + fit math + uniform packing below are kept in lockstep with
// `native_capture.rs`'s compositor; that module is macOS-`#[cfg]`-only so it
// can't be shared without a refactor that couldn't be verified against a macOS
// build from here (§5). Any change to the layer look must be made in BOTH.
// ============================================================================

use canvas_core::{Fit, LayerSource, TextureLayer};
use std::collections::HashMap;

/// WGSL for a layer blit — VERBATIM from `native_capture.rs` (keep in sync).
const LAYER_BLIT_WGSL: &str = r#"
struct Layer {
    uv: vec4<f32>,     // uv_scale.xy, uv_offset.xy
    geo: vec4<f32>,    // rect_w_px, rect_h_px, radius_px, opacity
    border: vec4<f32>, // border_width_px, use_src_alpha, _, _
    bcolor: vec4<f32>, // border r, g, b, a (0..1)
};
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> layer: Layer;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var out: VsOut;
    let xy = p[i];
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = vec2<f32>((xy.x + 1.0) * 0.5, (1.0 - xy.y) * 0.5);
    return out;
}

fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let suv = in.uv * layer.uv.xy + layer.uv.zw;
    let inside = all(suv >= vec2<f32>(0.0)) && all(suv <= vec2<f32>(1.0));
    let inb = select(0.0, 1.0, inside);
    let texel = textureSample(tex, samp, clamp(suv, vec2<f32>(0.0), vec2<f32>(1.0)));
    let col = texel.rgb;
    let size = layer.geo.xy;
    let radius = layer.geo.z;
    let opacity = layer.geo.w;
    let pp = (in.uv - vec2<f32>(0.5)) * size;
    let d = sd_round_box(pp, size * 0.5, radius);
    let aa = 1.0 - smoothstep(-1.0, 1.0, d);
    let use_src_alpha = layer.border.y;
    let src_a = mix(1.0, texel.a, use_src_alpha);
    var rgb = col;
    var a = aa * inb * opacity * src_a;
    let bw = layer.border.x;
    if (bw > 0.0) {
        let inner = 1.0 - smoothstep(-1.0, 1.0, d + bw);
        let bcov = clamp(aa - inner, 0.0, 1.0);
        rgb = mix(rgb, layer.bcolor.rgb, bcov);
        a = mix(a, layer.bcolor.a * opacity, bcov);
    }
    return vec4<f32>(rgb, a);
}
"#;

const LAYER_STRIDE: u64 = 256;
const MAX_LAYERS: usize = 16;
const MAX_CACHE: usize = 32;

/// A cached, GPU-resident copy of a stream layer's most recent CPU frame, keyed
/// by the layer's index. Re-uploaded only when `gen` advances or the size changes.
struct StreamTex {
    bind_group: wgpu::BindGroup,
    texture: wgpu::Texture,
    size: (u32, u32),
    gen: u64,
}

pub(crate) struct LayerCompositor {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    /// Latest uploaded frame per stream-layer index (camera/screen share).
    stream_cache: HashMap<usize, StreamTex>,
    /// Uploaded static images keyed by `ImageSource::id`; gen-invalidated.
    image_cache: HashMap<u64, (wgpu::BindGroup, wgpu::Texture, (u32, u32), u64)>,
    /// Reused frame scratch (taken/returned around `latest()` to dodge borrows).
    scratch: Vec<u8>,
}

impl LayerCompositor {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("layer-blit-shader"),
            source: wgpu::ShaderSource::Wgsl(LAYER_BLIT_WGSL.into()),
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer-blit-bgl"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: std::num::NonZeroU64::new(64),
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layer-blit-pl"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("layer-blit-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                // The vello target is Rgba8Unorm; alpha-blend so rounded corners +
                // letterbox + opacity reveal the strokes behind.
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("layer-blit-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("layer-blit-uniforms"),
            size: LAYER_STRIDE * MAX_LAYERS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            sampler,
            bind_layout,
            uniforms,
            stream_cache: HashMap::new(),
            image_cache: HashMap::new(),
            scratch: Vec::new(),
        }
    }

    /// Composite `layers` (in order) over the target — positioned, fit-cropped,
    /// rounded, opacity/border-blended. A stream layer uploads its latest CPU
    /// frame; a layer with no frame/image yet is skipped.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn composite_layers(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        layers: &[TextureLayer],
        target_view: &wgpu::TextureView,
        scale: f32,
        target_w: u32,
        target_h: u32,
    ) {
        for (i, layer) in layers.iter().enumerate().take(MAX_LAYERS) {
            // Resolve the layer's texture into the appropriate cache, then re-borrow
            // its bind group + size for the shared draw below.
            let resolved = match &layer.source {
                LayerSource::Stream(f) => {
                    let Some(stream) = f() else {
                        continue;
                    };
                    if !self.ensure_stream_frame(device, queue, i, &stream) {
                        continue;
                    }
                    Resolved::Stream(i)
                }
                LayerSource::Image(f) => {
                    let Some(img) = f() else {
                        continue;
                    };
                    if !img.is_valid() {
                        continue;
                    }
                    let stale = match self.image_cache.get(&img.id) {
                        Some((_, _, _, gen)) => *gen != img.generation,
                        None => true,
                    };
                    if stale {
                        if let Some(entry) = self.upload_image(device, queue, &img) {
                            if self.image_cache.len() >= MAX_CACHE {
                                self.image_cache.clear();
                            }
                            self.image_cache.insert(img.id, entry);
                        }
                    }
                    if !self.image_cache.contains_key(&img.id) {
                        continue;
                    }
                    Resolved::Image(img.id)
                }
            };

            let (bind_group, cam_w, cam_h) = match &resolved {
                Resolved::Stream(idx) => {
                    let st = self.stream_cache.get(idx).expect("just ensured");
                    (&st.bind_group, st.size.0, st.size.1)
                }
                Resolved::Image(id) => {
                    let (bg, _, (w, h), _) = self.image_cache.get(id).expect("just inserted");
                    (bg, *w, *h)
                }
            };
            // Image layers carry meaningful alpha; stream layers are opaque.
            let use_src_alpha = matches!(resolved, Resolved::Image(_)) as u32 as f32;

            let (lx, ly, lw, lh) = (layer.rect)();
            let (rx, ry, rw, rh) = (lx * scale, ly * scale, lw * scale, lh * scale);
            if rw < 1.0 || rh < 1.0 {
                continue;
            }
            let vx = rx.clamp(0.0, target_w as f32);
            let vy = ry.clamp(0.0, target_h as f32);
            let vw = (rx + rw).clamp(0.0, target_w as f32) - vx;
            let vh = (ry + rh).clamp(0.0, target_h as f32) - vy;
            if vw < 1.0 || vh < 1.0 {
                continue;
            }

            let (cx, cy, cw, ch) = layer.src_crop.unwrap_or((0.0, 0.0, 1.0, 1.0));
            let cropped_w = cam_w as f32 * cw.max(f32::EPSILON);
            let cropped_h = cam_h as f32 * ch.max(f32::EPSILON);
            let cam_aspect = cropped_w / cropped_h.max(1.0);
            let dst_aspect = vw / vh;
            let (fsx, fsy, fox, foy) = uv_transform(layer.fit, cam_aspect, dst_aspect);
            let (sx, sy, ox, oy) = (fsx * cw, fsy * ch, fox * cw + cx, foy * ch + cy);
            let radius_px = ((layer.corner_radius)() * scale).max(0.0);
            let border_px = (layer.border_width * scale).max(0.0);
            let bc = layer.border_color;
            let u = [
                sx, sy, ox, oy,
                vw, vh, radius_px, layer.opacity.clamp(0.0, 1.0),
                border_px, use_src_alpha, 0.0, 0.0,
                bc.r as f32 / 255.0, bc.g as f32 / 255.0, bc.b as f32 / 255.0, bc.a as f32 / 255.0,
            ];
            let mut bytes = [0u8; 64];
            for (j, f) in u.iter().enumerate() {
                bytes[j * 4..j * 4 + 4].copy_from_slice(&f.to_ne_bytes());
            }
            let offset = i as u64 * LAYER_STRIDE;
            queue.write_buffer(&self.uniforms, offset, &bytes);

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("layer-composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bind_group, &[offset as u32]);
            pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            pass.draw(0..3, 0..1);
        }
    }

    /// Ensure stream-layer `idx`'s latest CPU frame is uploaded to a cached
    /// texture. Returns `true` if a drawable texture is available (fresh or reused),
    /// `false` if the stream has produced no frame yet (skip the layer).
    fn ensure_stream_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        idx: usize,
        stream: &media_stream::MediaStream,
    ) -> bool {
        let gen = stream.generation();
        if let Some(st) = self.stream_cache.get(&idx) {
            if st.gen == gen {
                return true; // unchanged frame — reuse the cached texture
            }
        }
        // Pull the newest frame (take/return the scratch buffer so `latest` doesn't
        // conflict with the `&self` upload borrow).
        let mut buf = std::mem::take(&mut self.scratch);
        let dims = stream.latest(&mut buf);
        let ok = match dims {
            Some((w, h)) if w > 0 && h > 0 => {
                let reuse = self
                    .stream_cache
                    .get(&idx)
                    .map(|st| st.size == (w, h))
                    .unwrap_or(false);
                if reuse {
                    // Same size → overwrite the existing texture's pixels in place.
                    let st = self.stream_cache.get_mut(&idx).unwrap();
                    write_rgba(queue, &st.texture, w, h, &buf);
                    st.gen = gen;
                } else {
                    if self.stream_cache.len() >= MAX_CACHE {
                        self.stream_cache.clear();
                    }
                    let (bind_group, texture) = self.upload_new_stream(device, queue, w, h, &buf);
                    self.stream_cache
                        .insert(idx, StreamTex { bind_group, texture, size: (w, h), gen });
                }
                true
            }
            // No frame yet: reuse a prior texture if we have one, else skip.
            _ => self.stream_cache.contains_key(&idx),
        };
        self.scratch = buf;
        ok
    }

    /// Create a new `Rgba8Unorm` texture + bind group from a straight-`RGBA8` CPU
    /// frame (camera/screen). Straight, non-premultiplied — stream layers draw as
    /// opaque, so alpha is ignored downstream.
    fn upload_new_stream(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        w: u32,
        h: u32,
        rgba: &[u8],
    ) -> (wgpu::BindGroup, wgpu::Texture) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer-stream-frame"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        write_rgba(queue, &texture, w, h, rgba);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.bind_group_for(device, &view);
        (bind_group, texture)
    }

    /// Upload a static `ImageSource`'s straight-RGBA8 pixels — one-time, cached by
    /// id/generation (identical to the macOS compositor's `upload_image`).
    fn upload_image(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        img: &canvas_core::ImageSource,
    ) -> Option<(wgpu::BindGroup, wgpu::Texture, (u32, u32), u64)> {
        let (w, h) = (img.width, img.height);
        if w == 0 || h == 0 {
            return None;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer-image"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        write_rgba(queue, &texture, w, h, &img.rgba);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.bind_group_for(device, &view);
        Some((bind_group, texture, (w, h), img.generation))
    }

    fn bind_group_for(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer-bind-group"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.uniforms,
                        offset: 0,
                        size: std::num::NonZeroU64::new(64),
                    }),
                },
            ],
        })
    }
}

/// Write a tightly-packed straight-`RGBA8` frame into `texture` (stride = w*4).
fn write_rgba(queue: &wgpu::Queue, texture: &wgpu::Texture, w: u32, h: u32, rgba: &[u8]) {
    let needed = (w * h * 4) as usize;
    // Guard a short buffer (a torn/partial frame) rather than tripping wgpu's
    // copy-size validation.
    let src = if rgba.len() >= needed { &rgba[..needed] } else { return };
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        src,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
}

/// Which cache a resolved layer lives in, so the draw code re-borrows its bind
/// group after the (mutable) cache-fill step.
enum Resolved {
    Stream(usize),
    Image(u64),
}

/// UV scale + offset mapping the source into the destination rect for a `Fit`.
/// VERBATIM from `native_capture.rs` (keep in sync).
fn uv_transform(fit: Fit, cam_aspect: f32, dst_aspect: f32) -> (f32, f32, f32, f32) {
    match fit {
        Fit::Fill => (1.0, 1.0, 0.0, 0.0),
        Fit::Cover => {
            if cam_aspect > dst_aspect {
                let sx = dst_aspect / cam_aspect;
                (sx, 1.0, (1.0 - sx) * 0.5, 0.0)
            } else {
                let sy = cam_aspect / dst_aspect;
                (1.0, sy, 0.0, (1.0 - sy) * 0.5)
            }
        }
        Fit::Contain => {
            if cam_aspect > dst_aspect {
                let f = dst_aspect / cam_aspect;
                (1.0, 1.0 / f, 0.0, (f - 1.0) / (2.0 * f))
            } else {
                let f = cam_aspect / dst_aspect;
                (1.0 / f, 1.0, (f - 1.0) / (2.0 * f), 0.0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glow::HasContext;

    extern "C" {
        fn mmap(a: *mut c_void, l: usize, p: i32, f: i32, fd: i32, o: i64) -> *mut c_void;
        fn munmap(a: *mut c_void, l: usize) -> i32;
    }
    const PROT_READ: i32 = 0x1;
    const MAP_SHARED: i32 = 0x1;

    fn headless_gl() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::GL,
            flags: wgpu::InstanceFlags::empty(),
            memory_budget_thresholds: Default::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("native-capture-linux-test"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        }))
        .ok()
    }

    /// The regression test for the recording corruption (CLAUDE.md §8): render a
    /// spatially-varying 4-quadrant pattern into a REAL ring buffer produced by
    /// `make_pool_item`, then `mmap` the exported dma-buf fd and assert the pattern
    /// lands at the correct LINEAR byte offsets. A tiled export (the old, corrupt
    /// path) scrambles this; a genuinely-linear GBM buffer round-trips exactly. This
    /// exercises the real production code (GBM alloc → EGLImage import → wgpu wrap),
    /// including that the GL texture stays valid AFTER the EGLImage is destroyed.
    #[test]
    fn gpu_render_into_linear_dmabuf_reads_back_pixel_correct() {
        let Some((device, queue)) = headless_gl() else {
            eprintln!("SKIP: no headless GL device");
            return;
        };
        let Some(gbm) = Gbm::open().map(Rc::new) else {
            eprintln!("SKIP: no GBM device / render node");
            return;
        };
        let Some(egl) = egl::Egl::get() else {
            eprintln!("SKIP: EGL import entry points unavailable");
            return;
        };

        const W: u32 = 64;
        const H: u32 = 64;
        let Some(item) = make_pool_item(&device, &gbm, egl, W, H) else {
            panic!("make_pool_item returned None — linear export failed on this box");
        };
        let stride = item.frame.stride as u32;
        let fd = item.frame.fd;
        assert_eq!(item.frame.modifier, DRM_FORMAT_MOD_LINEAR, "must publish LINEAR");
        assert_eq!(item.frame.fourcc, DRM_FORMAT_ABGR8888);

        // Build a 4-quadrant pattern, upload to a source texture, blit into the ring
        // (the real `blit_into` path).
        let (tl, tr, bl, br) =
            ([200u8, 0, 0, 255], [0u8, 200, 0, 255], [0u8, 0, 200, 255], [200u8, 200, 200, 255]);
        let mut src = vec![0u8; (W * H * 4) as usize];
        for y in 0..H {
            for x in 0..W {
                let c = match (x < W / 2, y < H / 2) {
                    (true, true) => tl,
                    (false, true) => tr,
                    (true, false) => bl,
                    (false, false) => br,
                };
                let o = ((y * W + x) * 4) as usize;
                src[o..o + 4].copy_from_slice(&c);
            }
        }
        let src_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pattern-src"),
            size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &src_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &src,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(W * 4),
                rows_per_image: Some(H),
            },
            wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        );
        let src_view = src_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let blitter = wgpu::util::TextureBlitter::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        blitter.copy(&device, &mut enc, &src_view, &item.view);
        queue.submit([enc.finish()]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        // Make the GPU write to the dma-buf visible to the CPU mmap.
        {
            let hal = unsafe { device.as_hal::<wgpu_hal::api::Gles>() }.unwrap();
            let gl = hal.context().lock();
            unsafe { gl.finish() };
        }

        // mmap the dma-buf fd; assert the 4 quadrant colors at LINEAR offsets.
        let size = (stride * H) as usize;
        let map = unsafe { mmap(std::ptr::null_mut(), size, PROT_READ, MAP_SHARED, fd, 0) };
        assert!(map != usize::MAX as *mut c_void && !map.is_null(), "mmap failed");
        let bytes = unsafe { std::slice::from_raw_parts(map as *const u8, size) };
        let sample = |x: u32, y: u32| -> [u8; 4] {
            let o = (y * stride + x * 4) as usize;
            [bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]
        };
        let (s_tl, s_tr, s_bl, s_br) = (
            sample(W / 4, H / 4),
            sample(3 * W / 4, H / 4),
            sample(W / 4, 3 * H / 4),
            sample(3 * W / 4, 3 * H / 4),
        );
        unsafe { munmap(map, size) };
        eprintln!("linear read-back: TL={s_tl:?} TR={s_tr:?} BL={s_bl:?} BR={s_br:?}");

        let near =
            |a: [u8; 4], b: [u8; 4]| a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() <= 3);
        assert!(
            near(s_tl, tl) && near(s_tr, tr) && near(s_bl, bl) && near(s_br, br),
            "pattern did not round-trip through the LINEAR dma-buf: \
             TL={s_tl:?}(want {tl:?}) TR={s_tr:?}(want {tr:?}) \
             BL={s_bl:?}(want {bl:?}) BR={s_br:?}(want {br:?})"
        );
        // `item` drops here → PoolItem::Drop destroys the bo; fd closes.
    }

    /// Regression (CLAUDE.md §7/§8): a live camera `MediaStream` layer must be
    /// COMPOSITED INTO the canvas target on Linux (same as macOS), not shown via a
    /// separate widget — so it appears on-screen AND in recordings. Composite a
    /// solid-red camera frame over a blue target and assert the target center turns
    /// red. Fully headless on the wgpu GL backend.
    #[test]
    fn camera_stream_layer_composites_into_the_canvas_target() {
        use std::rc::Rc;
        let Some((device, queue)) = headless_gl() else {
            eprintln!("SKIP: no headless GL device");
            return;
        };
        const W: u32 = 64;
        const H: u32 = 64;

        // Target = the vello canvas texture; start it solid BLUE.
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas-target"),
            size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let _rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear-blue"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }

        // A camera producing one solid-RED frame.
        let (stream, writer) = media_stream::MediaStream::new();
        let red: Vec<u8> = (0..(W * H)).flat_map(|_| [220u8, 0, 0, 255]).collect();
        writer.write_rgba8(W, H, &red);

        // Full-target Fill layer (no rounding/border) so the whole target is covered.
        let stream_for_layer = stream.clone();
        let layer = TextureLayer::new(
            Rc::new(move || Some(stream_for_layer.clone())),
            Rc::new(|| (0.0, 0.0, W as f32, H as f32)),
        )
        .fit(Fit::Fill);

        let mut compositor = LayerCompositor::new(&device);
        compositor.composite_layers(&device, &queue, &mut enc, &[layer], &target_view, 1.0, W, H);
        queue.submit([enc.finish()]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());

        // Read back the center pixel: it must be the camera RED, not the blue bg —
        // proving the stream composited into the canvas.
        let bpr = (W * 4).next_multiple_of(256);
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (bpr * H) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(H),
                },
            },
            wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let data = slice.get_mapped_range();
        let o = (H / 2 * bpr + W / 2 * 4) as usize;
        let px = [data[o], data[o + 1], data[o + 2], data[o + 3]];
        eprintln!("composited center pixel = {px:?} (expect ~red [220,0,0,255])");
        assert!(px[0] > 180 && px[1] < 40 && px[2] < 40, "camera frame did not composite: {px:?}");
    }
}
