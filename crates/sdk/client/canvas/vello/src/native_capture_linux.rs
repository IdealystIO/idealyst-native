//! Linux zero-copy capture target for canvas-vello — the dma-buf analog of the
//! macOS IOSurface ring in [`native_capture`](super::native_capture) (proven end
//! to end by `tests/dmabuf_export_spike.rs`).
//!
//! A small ring of `Rgba8Unorm` wgpu textures, each **exported once** as a
//! **dma-buf** through `EGL_MESA_image_dma_buf_export`. Each render blits the vello
//! target into the next ring texture and publishes that texture's [`DmaBufFrame`]
//! descriptor to the stream's native source. A GPU-rendered frame therefore stays a
//! GPU handle through the whole stream — **no CPU read-back and no swizzle in
//! canvas-vello**. A consumer that needs the pixels on the CPU (the software VP8
//! encoder) downloads them at ITS boundary (GStreamer `gldownload`), not here; a
//! hardware (VAAPI) encoder would consume the dma-buf with zero download. That is
//! the whole point of the native-source contract.
//!
//! # Why the ring texture must be a real GL texture, not a renderbuffer
//!
//! `EGL_GL_TEXTURE_2D_KHR` can only wrap a `GL_TEXTURE_2D` name. wgpu-hal's GLES
//! backend allocates a *renderbuffer* (not exportable) for a texture whose usage is
//! only render-attachment-ish; adding `TEXTURE_BINDING` forces a real texture. The
//! spike asserts this held; the ring here uses `RENDER_ATTACHMENT | TEXTURE_BINDING`
//! for the same reason (RENDER_ATTACHMENT so the blitter can draw into it).
//!
//! # fd lifetime
//!
//! Each ring [`PoolItem`] owns its exported dma-buf fd for its whole lifetime (an
//! [`OwnedFd`], closed on drop / pool rebuild). The published [`DmaBufFrame`]
//! carries a **borrowed** `RawFd` into that owned fd — valid as long as the ring
//! item lives (the recording's lifetime). The consumer `dup(2)`s it when it wraps
//! the fd into a `GstMemory`. See `media_stream::dmabuf`.
//!
//! # Why a ring (not one texture)
//!
//! The consumer (GStreamer `appsrc`) reads the dma-buf asynchronously on its own
//! thread. Rendering the next frame into a DIFFERENT ring texture means the canvas
//! never overwrites the buffer the encoder is still reading. At the canvas's ~60fps
//! capture cadence the GPU blit for frame N completes microseconds after submit —
//! long before the texture is reused `POOL` frames later — so the cadence is the
//! sync (no explicit fence), exactly as on macOS.
//!
//! EGL entry points are resolved through libepoxy's `eglGetProcAddress` (the same
//! loader `GlTarget::get_proc_address` / `backend-linux`'s `gl_loader` use), NOT a
//! separate `libEGL` link — see [`egl`].

use media_stream::{DmaBufFrame, FrameWriter};
use std::os::fd::{FromRawFd, OwnedFd};

/// Textures in the ring. 3 = standard triple-buffering: enough that the encoder is
/// never reading the texture the canvas is rendering into.
const POOL: usize = 3;

struct PoolItem {
    /// Owns the exported dma-buf fd for the item's lifetime (closed on drop).
    _fd: OwnedFd,
    /// wgpu view of the ring texture (the blit's render target).
    view: wgpu::TextureView,
    _texture: wgpu::Texture,
    /// The published descriptor — its `fd` borrows `_fd`.
    frame: DmaBufFrame,
}

/// The canvas's Linux native-capture ring. Lazily (re)built to match the drawable
/// size; idle (empty pool) until a recorder taps the stream.
pub(crate) struct NativeCapture {
    writer: FrameWriter,
    pool: Vec<PoolItem>,
    next: usize,
    size: (u32, u32),
    /// `Rgba8Unorm`→`Rgba8Unorm` straight-copy blitter (built lazily). The vello
    /// target is already `Rgba8Unorm`, so this is a plain GPU copy — no swizzle.
    blitter: Option<wgpu::util::TextureBlitter>,
}

/// Whether the dma-buf zero-copy capture path is enabled. OFF by default —
/// opt in with `IDEALYST_CANVAS_DMABUF=1`. See `NativeCapture::wants` for why
/// (tiled implicit-modifier import corruption). Read once, cached.
fn dmabuf_enabled() -> bool {
    use std::sync::OnceLock;
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("IDEALYST_CANVAS_DMABUF").map(|v| v == "1" || v == "true").unwrap_or(false)
    })
}

impl NativeCapture {
    pub(crate) fn new(writer: FrameWriter) -> Self {
        Self { writer, pool: Vec::new(), next: 0, size: (0, 0), blitter: None }
    }

    /// True only while a recorder holds a `NativeTap` on the stream's dma-buf
    /// source — gates all the GPU export work so an un-recorded canvas pays nothing.
    ///
    /// GATED OFF by default (opt in with `IDEALYST_CANVAS_DMABUF=1`). The dma-buf
    /// zero-copy path is implemented and the EGL export is proven, but a wgpu
    /// `RENDER_ATTACHMENT` texture on Mesa is GPU-*tiled* and exported with an
    /// *implicit* modifier (`DRM_FORMAT_MOD_INVALID`); importing that as linear in
    /// `glupload` produces corrupt frames (the recorded video was garbled). Until
    /// that tiling handshake is solved (force-linear export, or a correct
    /// implicit-modifier EGL import), `wants()` stays false so recording uses the
    /// GPU→CPU read-back — one copy, but CORRECT pixels. Returning false here makes
    /// `render()` take the read-back branch (it skips read-back only when native is
    /// active), so a corrupt zero-copy frame never reaches the encoder.
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
    /// source. Call after the GPU submit so the blit is in flight; the ring
    /// guarantees this texture isn't reused until `POOL` frames later.
    pub(crate) fn publish(&self, idx: usize) {
        self.writer.publish_dmabuf(self.pool[idx].frame);
    }

    /// (Re)build the ring when the drawable size changes (or on first use). Runs
    /// with the GL context current (the caller's `render()` made it current), which
    /// EGL export requires. A build failure (export unsupported, no EGL) leaves the
    /// pool empty, so `blit_into` returns `None` and the canvas falls back to the
    /// CPU read-back path — never a hard error.
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
        let Some(egl) = egl::Egl::get() else {
            // No EGL export entry points (not Mesa / no EGL_MESA_image_dma_buf_export)
            // — stay on CPU read-back.
            return;
        };
        for _ in 0..POOL {
            let Some(item) = make_pool_item(device, egl, w, h) else {
                self.pool.clear();
                return;
            };
            self.pool.push(item);
        }
    }
}

/// Create one ring texture and export it as a dma-buf. `None` (→ CPU fallback) if
/// wgpu backed it with a renderbuffer, the GL name can't be read, or EGL export
/// fails.
fn make_pool_item(
    device: &wgpu::Device,
    egl: &egl::Egl,
    w: u32,
    h: u32,
) -> Option<PoolItem> {
    // RENDER_ATTACHMENT so the blitter draws into it; TEXTURE_BINDING forces a real
    // GL_TEXTURE_2D (not a renderbuffer), the only thing EGL_GL_TEXTURE_2D_KHR can
    // wrap. `Rgba8Unorm` = the vello target format.
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("canvas-vello-dmabuf-ring"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let gl_name = raw_gl_texture_name(&texture)?;
    let export = unsafe { egl.export_texture(gl_name) }?;

    // SAFETY: `export.fd` is a fresh dma-buf fd the driver dup'd for us; the ring
    // item now owns it and closes it on drop.
    let owned = unsafe { OwnedFd::from_raw_fd(export.fd) };
    let frame = DmaBufFrame {
        // Borrows `owned`; valid while this PoolItem lives (see the module docs).
        fd: export.fd,
        width: w,
        height: h,
        fourcc: export.fourcc,
        stride: export.stride,
        offset: export.offset,
        modifier: export.modifier,
    };
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Some(PoolItem { _fd: owned, view, _texture: texture, frame })
}

/// Read the raw `GL_TEXTURE_2D` name out of a wgpu texture via wgpu-hal's GLES
/// `TextureInner`. `None` if wgpu backed it with a renderbuffer (see the module
/// docs) — wgpu's public API exposes no other route to the name.
fn raw_gl_texture_name(texture: &wgpu::Texture) -> Option<u32> {
    let hal = unsafe { texture.as_hal::<wgpu_hal::api::Gles>() }?;
    match &hal.inner {
        wgpu_hal::gles::TextureInner::Texture { raw, .. } => Some(raw.0.get()),
        _ => None,
    }
}

// ============================================================================
// EGL dma-buf export — resolved through libepoxy's eglGetProcAddress, the same
// loader the GL adoption path uses. No separate libEGL link.
// ============================================================================

mod egl {
    use std::ffi::{c_void, CString};
    use std::sync::OnceLock;

    // EGL types (the subset the export needs).
    type EglDisplay = *mut c_void;
    type EglContext = *mut c_void;
    type EglImage = *mut c_void;
    type EglClientBuffer = *mut c_void;
    type EglInt = i32;
    type EglEnum = u32;
    type EglBoolean = u32;

    const EGL_TRUE: EglBoolean = 1;
    /// `EGL_GL_TEXTURE_2D_KHR` — wrap a GL texture name as an `EGLImage`.
    const EGL_GL_TEXTURE_2D_KHR: EglEnum = 0x30B1;

    type FnGetProcAddress = unsafe extern "C" fn(*const std::ffi::c_char) -> *const c_void;
    type FnGetCurrentDisplay = unsafe extern "C" fn() -> EglDisplay;
    type FnGetCurrentContext = unsafe extern "C" fn() -> EglContext;
    type FnCreateImageKHR = unsafe extern "C" fn(
        EglDisplay,
        EglContext,
        EglEnum,
        EglClientBuffer,
        *const EglInt,
    ) -> EglImage;
    type FnDestroyImageKHR = unsafe extern "C" fn(EglDisplay, EglImage) -> EglBoolean;
    type FnExportQueryMESA = unsafe extern "C" fn(
        EglDisplay,
        EglImage,
        *mut EglInt, // fourcc
        *mut EglInt, // num_planes
        *mut u64,    // modifiers
    ) -> EglBoolean;
    type FnExportMESA = unsafe extern "C" fn(
        EglDisplay,
        EglImage,
        *mut EglInt, // fds
        *mut EglInt, // strides
        *mut EglInt, // offsets
    ) -> EglBoolean;

    /// The result of exporting one GL texture as a single-plane dma-buf.
    pub(super) struct Exported {
        pub fd: i32,
        pub fourcc: u32,
        pub stride: i32,
        pub offset: i32,
        pub modifier: u64,
    }

    /// Resolved EGL export entry points. Built once (they're process-global).
    pub(super) struct Egl {
        get_current_display: FnGetCurrentDisplay,
        get_current_context: FnGetCurrentContext,
        create_image: FnCreateImageKHR,
        destroy_image: FnDestroyImageKHR,
        export_query: FnExportQueryMESA,
        export: FnExportMESA,
    }

    // The resolved `fn` pointers are process-global and thread-safe to hold; they
    // are only ever *called* on the render thread with the GL context current.
    unsafe impl Send for Egl {}
    unsafe impl Sync for Egl {}

    static EGL: OnceLock<Option<Egl>> = OnceLock::new();

    impl Egl {
        /// Resolve the EGL export entry points once, or `None` if unavailable (no
        /// libepoxy, no `EGL_MESA_image_dma_buf_export`).
        pub(super) fn get() -> Option<&'static Egl> {
            EGL.get_or_init(Egl::resolve).as_ref()
        }

        fn resolve() -> Option<Egl> {
            let get_proc = epoxy_egl_get_proc_address()?;
            // SAFETY: each name is resolved to a fn pointer of the matching EGL
            // signature; a null result (unavailable) short-circuits to `None`.
            unsafe {
                Some(Egl {
                    get_current_display: transmute_fn(get_proc, "eglGetCurrentDisplay")?,
                    get_current_context: transmute_fn(get_proc, "eglGetCurrentContext")?,
                    create_image: transmute_fn(get_proc, "eglCreateImageKHR")?,
                    destroy_image: transmute_fn(get_proc, "eglDestroyImageKHR")?,
                    export_query: transmute_fn(get_proc, "eglExportDMABUFImageQueryMESA")?,
                    export: transmute_fn(get_proc, "eglExportDMABUFImageMESA")?,
                })
            }
        }

        /// Export GL texture `gl_name` (current context) as a single-plane dma-buf.
        /// `None` on any EGL failure (caller falls back to CPU read-back).
        ///
        /// # Safety
        /// The GL context that owns `gl_name` must be current on this thread.
        pub(super) unsafe fn export_texture(&self, gl_name: u32) -> Option<Exported> {
            let dpy = (self.get_current_display)();
            let ctx = (self.get_current_context)();
            if dpy.is_null() || ctx.is_null() {
                return None;
            }
            let image = (self.create_image)(
                dpy,
                ctx,
                EGL_GL_TEXTURE_2D_KHR,
                gl_name as usize as EglClientBuffer,
                std::ptr::null(),
            );
            if image.is_null() {
                return None;
            }

            let mut fourcc: EglInt = 0;
            let mut num_planes: EglInt = 0;
            let mut modifier: u64 = 0;
            if (self.export_query)(dpy, image, &mut fourcc, &mut num_planes, &mut modifier)
                != EGL_TRUE
                || num_planes != 1
            {
                // Single-plane only (an RGBA texture is one plane); anything else is
                // outside this path's contract.
                (self.destroy_image)(dpy, image);
                return None;
            }

            let mut fd: EglInt = -1;
            let mut stride: EglInt = 0;
            let mut offset: EglInt = 0;
            let ok = (self.export)(dpy, image, &mut fd, &mut stride, &mut offset) == EGL_TRUE;
            // The image can be destroyed now; the exported fd owns the underlying
            // buffer independently (the driver dup'd it).
            (self.destroy_image)(dpy, image);
            if !ok || fd < 0 {
                if fd >= 0 {
                    // Balance a stray fd if the driver returned one despite failing.
                    unsafe { close(fd) };
                }
                return None;
            }

            Some(Exported {
                fd,
                fourcc: fourcc as u32,
                stride,
                offset,
                // Pass Mesa's reported modifier through verbatim: on this stack it's
                // `DRM_FORMAT_MOD_INVALID` (0x00ff_ffff_ffff_ffff, no explicit
                // modifier — the common case), but 0 (`DRM_FORMAT_MOD_LINEAR`) and
                // real tiling modifiers are all legitimate and the consumer must see
                // exactly what the driver produced to negotiate the import.
                modifier,
            })
        }
    }

    /// Resolve `symbol` through `get_proc` and transmute to the fn pointer type `T`.
    /// `None` if the loader returns null.
    unsafe fn transmute_fn<T: Copy>(get_proc: FnGetProcAddress, symbol: &str) -> Option<T> {
        let cname = CString::new(symbol).ok()?;
        let p = unsafe { get_proc(cname.as_ptr()) };
        if p.is_null() {
            return None;
        }
        Some(unsafe { *(&p as *const *const c_void as *const T) })
    }

    /// Read libepoxy's `epoxy_eglGetProcAddress` dispatch pointer — the same route
    /// `backend-linux`'s `gl_loader` uses. libepoxy exports GL/EGL loaders as DATA
    /// symbols (a lazily-resolved dispatch pointer), so this is `dlsym` for the
    /// variable's address followed by one deref. GTK already mapped libepoxy;
    /// `dlopen` just bumps its refcount and is never `dlclose`d (the pointer must
    /// stay valid for the process lifetime).
    fn epoxy_egl_get_proc_address() -> Option<FnGetProcAddress> {
        static PTR: OnceLock<Option<usize>> = OnceLock::new();
        let raw = (*PTR.get_or_init(|| unsafe {
            let mut handle = libc::dlopen(c"libepoxy.so.0".as_ptr(), libc::RTLD_LAZY);
            if handle.is_null() {
                handle = libc::dlopen(c"libepoxy.so".as_ptr(), libc::RTLD_LAZY);
            }
            if handle.is_null() {
                return None;
            }
            let addr = libc::dlsym(handle, c"epoxy_eglGetProcAddress".as_ptr());
            if addr.is_null() {
                return None;
            }
            // `dlsym` on a data symbol yields the ADDRESS OF the variable; the
            // function pointer is the value stored there, so deref once.
            let fnptr = *(addr as *const *const c_void);
            if fnptr.is_null() {
                None
            } else {
                Some(fnptr as usize)
            }
        }))?;
        Some(unsafe { std::mem::transmute::<*const c_void, FnGetProcAddress>(raw as *const c_void) })
    }

    /// `close(2)` — only the stray-fd cleanup path uses it.
    unsafe fn close(fd: i32) {
        extern "C" {
            fn close(fd: i32) -> i32;
        }
        unsafe {
            close(fd);
        }
    }
}

// ============================================================================
// LayerCompositor: Linux camera/screen layers are shown via an overlay `video`
// widget (not a GPU composite into the canvas target), so this stays a no-op —
// identical to the non-macOS stub. render.rs imports it uniformly.
// ============================================================================

pub(crate) struct LayerCompositor;

impl LayerCompositor {
    pub(crate) fn new(_device: &wgpu::Device) -> Self {
        LayerCompositor
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn composite_layers(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _encoder: &mut wgpu::CommandEncoder,
        _layers: &[canvas_core::TextureLayer],
        _target_view: &wgpu::TextureView,
        _scale: f32,
        _target_w: u32,
        _target_h: u32,
    ) {
    }
}
