//! STEP 0 spike (gating): prove the zero-copy seam on Linux — export a **wgpu-GL**
//! texture as a **dma-buf** through `EGL_MESA_image_dma_buf_export`, exactly as the
//! producer (`native_capture_linux`) will each frame. This is the Linux analog of
//! `iosurface_zerocopy_spike.rs` (macOS): if it yields a valid dma-buf fd + stride +
//! fourcc for a wgpu texture, vello can hand the encoder a GPU handle with no CPU
//! readback in canvas-vello.
//!
//! # Why the genuinely-uncertain piece is proven here, not reasoned about
//!
//! wgpu's *public* API never exposes the raw GL texture name, and EGL export needs
//! the EGLDisplay+context that GTK's GL area is current on. Three things could each
//! break the seam and none is inspectable:
//!
//! 1. `wgpu-hal`'s GLES `TextureInner::Texture { raw }` must actually hold a real
//!    `GL_TEXTURE_2D` name — but wgpu makes a *renderbuffer* (not exportable via
//!    `EGL_GL_TEXTURE_2D_KHR`) for a RENDER_ATTACHMENT-only texture. Adding
//!    `TEXTURE_BINDING` forces a texture; this asserts that held.
//! 2. `eglGetProcAddress` (epoxy's loader — the SAME one wgpu adopts through) must
//!    resolve the `EGL_MESA_image_dma_buf_export` entry points AND the core EGL
//!    `eglGetCurrent*` — not a separate `libEGL` link.
//! 3. Mesa must actually export the fd for a texture allocated by wgpu on the GL
//!    context GTK lent.
//!
//! # Why it's `#[ignore]` + interactive
//!
//! Needs a live `GtkGLArea` on a real GPU/EGL display (the adoption test's rig),
//! which a headless CI box lacks. Skips (returns) rather than fails when no display
//! or no GL is available, so it never breaks CI; run it on a workstation with
//! `cargo test -p canvas-vello --test dmabuf_export_spike -- --ignored --nocapture`.

#![cfg(target_os = "linux")]

use backend_linux::gtk4;
use gtk4::prelude::*;
use runtime_core::primitives::graphics::{GlTarget, GraphicsTarget, OnReadyEvent};
use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

// --- EGL types + constants (the subset the dma-buf export needs) -------------
type EglDisplay = *mut c_void;
type EglContext = *mut c_void;
type EglImage = *mut c_void;
type EglClientBuffer = *mut c_void;
type EglInt = i32;
type EglEnum = u32;
type EglBoolean = u32;

const EGL_TRUE: EglBoolean = 1;
const EGL_NO_CONTEXT: EglContext = std::ptr::null_mut();
const EGL_NO_DISPLAY: EglDisplay = std::ptr::null_mut();
const EGL_NO_IMAGE: EglImage = std::ptr::null_mut();
/// `EGL_GL_TEXTURE_2D_KHR` — wrap a GL texture name as an `EGLImage`.
const EGL_GL_TEXTURE_2D_KHR: EglEnum = 0x30B1;

/// A resolved outcome, carried out of `on_ready` for the test body to assert on.
struct Outcome {
    /// The wgpu texture became a real GL texture (name != 0), not a renderbuffer.
    gl_texture_name: Option<u32>,
    /// Every EGL entry point resolved through the GL loader.
    egl_resolved: bool,
    /// `eglCreateImageKHR` returned a non-null image for the texture.
    image_ok: bool,
    /// The successful `eglExportDMABUFImageMESA` result: fourcc, planes, fd, stride,
    /// offset, modifier.
    export: Option<DmaBuf>,
    /// A step failed AFTER adoption (a real problem, not an environment skip).
    fail: Option<String>,
    /// No display / no GL context — skip, don't fail.
    skip: Option<String>,
}

#[derive(Debug)]
struct DmaBuf {
    fourcc: u32,
    num_planes: i32,
    fd: i32,
    stride: i32,
    offset: i32,
    modifier: u64,
}

/// Resolve an EGL entry point of type `T` (a fn pointer) through the lent GL
/// loader — the epoxy `eglGetProcAddress` path, NOT a separate `libEGL` link.
fn egl_fn<T: Copy>(target: &GlTarget, symbol: &str) -> Option<T> {
    let p = target.get_proc_address(symbol);
    if p.is_null() {
        return None;
    }
    Some(unsafe { *(&p as *const *const c_void as *const T) })
}

/// Adopt the lent GL context through wgpu's GLES HAL — identical to
/// `gl_target_adoption.rs::adopt` / `render.rs::adopt`. Context must be current.
unsafe fn adopt(target: &GlTarget) -> Option<(wgpu::Device, wgpu::Queue)> {
    let exposed = unsafe {
        wgpu_hal::gles::Adapter::new_external(
            |sym| target.get_proc_address(sym),
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
    let adapter = unsafe { instance.create_adapter_from_hal(exposed) };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("dmabuf-export-spike"),
        required_features: wgpu::Features::empty(),
        required_limits: adapter.limits(),
        memory_hints: wgpu::MemoryHints::default(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        trace: wgpu::Trace::Off,
    }))
    .ok()?;
    Some((device, queue))
}

/// Read the raw `GL_TEXTURE_2D` name out of a wgpu texture via `wgpu-hal`'s GLES
/// `TextureInner`. `None` if wgpu backed it with a renderbuffer (which cannot be
/// exported as `EGL_GL_TEXTURE_2D_KHR`) — the whole reason we add TEXTURE_BINDING.
fn gl_texture_name(texture: &wgpu::Texture) -> Option<u32> {
    let hal = unsafe { texture.as_hal::<wgpu_hal::api::Gles>() }?;
    match &hal.inner {
        wgpu_hal::gles::TextureInner::Texture { raw, .. } => Some(raw.0.get()),
        _ => None,
    }
}

fn run_export(target: &GlTarget, device: &wgpu::Device, queue: &wgpu::Queue) -> Outcome {
    let mut o = Outcome {
        gl_texture_name: None,
        egl_resolved: false,
        image_ok: false,
        export: None,
        fail: None,
        skip: None,
    };

    const W: u32 = 64;
    const H: u32 = 48;

    // A real GL texture: RENDER_ATTACHMENT (vello blits into it) + TEXTURE_BINDING
    // (forces wgpu to allocate a GL_TEXTURE_2D, not a renderbuffer). Rgba8Unorm =
    // the vello target format.
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("dmabuf-export-target"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    // Clear it to a known color so the allocation is real GPU-backed storage.
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let _rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
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
    queue.submit([enc.finish()]);
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    // wgpu leaves ITS context state; re-establish GTK's so the GL name is valid
    // and current for the EGL calls below.
    target.make_current();

    let Some(name) = gl_texture_name(&texture) else {
        o.fail = Some("wgpu backed the texture with a renderbuffer, not a GL texture \
             — cannot export as EGL_GL_TEXTURE_2D_KHR (add TEXTURE_BINDING)".into());
        return o;
    };
    o.gl_texture_name = Some(name);

    // Resolve every EGL entry point through the GL loader (epoxy's
    // eglGetProcAddress) — no separate libEGL link.
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
        *mut EglInt,   // fourcc
        *mut EglInt,   // num_planes
        *mut u64,      // modifiers
    ) -> EglBoolean;
    type FnExportMESA = unsafe extern "C" fn(
        EglDisplay,
        EglImage,
        *mut EglInt, // fds
        *mut EglInt, // strides
        *mut EglInt, // offsets
    ) -> EglBoolean;

    let (
        Some(get_dpy),
        Some(get_ctx),
        Some(create_image),
        Some(destroy_image),
        Some(export_query),
        Some(export),
    ) = (
        egl_fn::<FnGetCurrentDisplay>(target, "eglGetCurrentDisplay"),
        egl_fn::<FnGetCurrentContext>(target, "eglGetCurrentContext"),
        egl_fn::<FnCreateImageKHR>(target, "eglCreateImageKHR"),
        egl_fn::<FnDestroyImageKHR>(target, "eglDestroyImageKHR"),
        egl_fn::<FnExportQueryMESA>(target, "eglExportDMABUFImageQueryMESA"),
        egl_fn::<FnExportMESA>(target, "eglExportDMABUFImageMESA"),
    ) else {
        o.fail = Some("an EGL entry point did not resolve through the GL loader \
             (eglGetCurrent*/eglCreateImageKHR/eglExportDMABUFImage*MESA)".into());
        return o;
    };
    o.egl_resolved = true;

    unsafe {
        let dpy = get_dpy();
        let ctx = get_ctx();
        if dpy == EGL_NO_DISPLAY || ctx == EGL_NO_CONTEXT {
            o.fail = Some(format!("no current EGL display/context (dpy={dpy:?} ctx={ctx:?}) \
                 — is GTK on EGL, not GLX?"));
            return o;
        }

        // Wrap the GL texture as an EGLImage. `attrib_list = EGL_NONE` (null-ok on
        // Mesa) — no level/z attributes needed for a 2D base level.
        let image = create_image(
            dpy,
            ctx,
            EGL_GL_TEXTURE_2D_KHR,
            name as usize as EglClientBuffer,
            std::ptr::null(),
        );
        if image == EGL_NO_IMAGE {
            o.fail = Some("eglCreateImageKHR returned EGL_NO_IMAGE for the GL texture".into());
            return o;
        }
        o.image_ok = true;

        let mut fourcc: EglInt = 0;
        let mut num_planes: EglInt = 0;
        let mut modifier: u64 = 0;
        if export_query(dpy, image, &mut fourcc, &mut num_planes, &mut modifier) != EGL_TRUE {
            destroy_image(dpy, image);
            o.fail = Some("eglExportDMABUFImageQueryMESA failed".into());
            return o;
        }

        // One plane for a single-plane RGBA texture; export its fd/stride/offset.
        let mut fd: EglInt = -1;
        let mut stride: EglInt = 0;
        let mut offset: EglInt = 0;
        if export(dpy, image, &mut fd, &mut stride, &mut offset) != EGL_TRUE {
            destroy_image(dpy, image);
            o.fail = Some("eglExportDMABUFImageMESA failed".into());
            return o;
        }

        // The image can be destroyed now; the exported fd owns the underlying
        // buffer independently (dup'd by the driver).
        destroy_image(dpy, image);

        if fd < 0 {
            o.fail = Some(format!("export returned an invalid fd ({fd})"));
            return o;
        }

        o.export = Some(DmaBuf {
            fourcc: fourcc as u32,
            num_planes,
            fd,
            stride,
            offset,
            modifier,
        });
        // Close the fd — the spike only proves the export works; the real producer
        // wraps it in an OwnedFd and hands it to GStreamer.
        libc_close(fd);
    }

    o
}

/// Close a raw fd without pulling `libc` as a dep — `close(2)` via the C library
/// GTK already links. Spike-only cleanup.
fn libc_close(fd: i32) {
    extern "C" {
        fn close(fd: i32) -> i32;
    }
    unsafe {
        close(fd);
    }
}

#[test]
#[ignore = "interactive: needs a live GtkGLArea on a real GPU/EGL display"]
fn wgpu_gl_texture_exports_as_a_dmabuf() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let outcome: Rc<RefCell<Option<Outcome>>> = Rc::new(RefCell::new(None));
    let out = outcome.clone();

    let on_ready = Box::new(move |ev: OnReadyEvent| {
        let GraphicsTarget::Gl(target) = &ev.target else {
            *out.borrow_mut() = Some(Outcome {
                gl_texture_name: None,
                egl_resolved: false,
                image_ok: false,
                export: None,
                fail: None,
                skip: Some("backend handed back a RawWindow target, not Gl".into()),
            });
            return;
        };
        target.make_current();
        let Some((device, queue)) = (unsafe { adopt(target) }) else {
            *out.borrow_mut() = Some(Outcome {
                gl_texture_name: None,
                egl_resolved: false,
                image_ok: false,
                export: None,
                fail: None,
                skip: Some("wgpu could not adopt the GL context".into()),
            });
            return;
        };
        *out.borrow_mut() = Some(run_export(target, &device, &queue));
    });

    let area =
        backend_linux::build_gl_area_for_test(on_ready, Box::new(|_| {}), Box::new(|| {}));
    area.set_size_request(64, 48);

    let window = gtk4::Window::new();
    window.set_default_size(64, 48);
    window.set_child(Some(&area));
    window.present();

    let ctx = gtk4::glib::MainContext::default();
    for _ in 0..10_000 {
        if outcome.borrow().is_some() {
            break;
        }
        ctx.iteration(false);
    }

    let Some(o) = outcome.borrow_mut().take() else {
        eprintln!("SKIP: on_ready never fired (no GL context in this environment)");
        return;
    };

    if let Some(reason) = &o.skip {
        eprintln!("SKIP: {reason}");
        return;
    }

    eprintln!(
        "[dmabuf-spike] gl_texture_name={:?} egl_resolved={} image_ok={} export={:?}",
        o.gl_texture_name, o.egl_resolved, o.image_ok, o.export
    );

    if let Some(fail) = &o.fail {
        panic!("dma-buf export failed: {fail}");
    }

    let export = o.export.expect("export must be Some when no failure recorded");
    assert!(export.fd >= 0, "exported fd must be valid, got {}", export.fd);
    assert!(export.stride > 0, "stride must be positive, got {}", export.stride);
    assert!(export.fourcc != 0, "fourcc must be non-zero");
    assert!(export.num_planes >= 1, "expected at least one plane");
    eprintln!(
        "[dmabuf-spike] SUCCESS — fourcc=0x{:08x} ('{}') planes={} fd={} stride={} offset={} modifier=0x{:016x}",
        export.fourcc,
        fourcc_str(export.fourcc),
        export.num_planes,
        export.fd,
        export.stride,
        export.offset,
        export.modifier,
    );
}

/// Decode a DRM fourcc into its 4 ASCII chars for the log line.
fn fourcc_str(fourcc: u32) -> String {
    let b = fourcc.to_le_bytes();
    b.iter().map(|&c| c as char).collect()
}
