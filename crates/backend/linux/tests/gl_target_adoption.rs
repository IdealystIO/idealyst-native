//! Proof that the `GlTarget` the GTK backend hands to `on_ready` is
//! actually adoptable by a real GPU library, and that pixels rendered
//! through it land in the `GtkGLArea`'s framebuffer the right way up.
//!
//! # Why this test exists
//!
//! `runtime_core::primitives::graphics::GraphicsTarget::Gl` is a
//! contract with three unverifiable-by-inspection claims:
//!
//! 1. The proc-address loader resolves enough of GL for a GPU library
//!    to build a working function table (see `src/gl_loader.rs` — Mesa
//!    hands back a synthesized stub for *any* GL-shaped name, so a
//!    table that looks complete can still be hollow).
//! 2. `make_current()` really does leave both the context current and
//!    the area's own framebuffer bound, so `framebuffer()` names the
//!    area's FBO rather than the default one.
//! 3. Rendering into that FBO with wgpu produces a correctly-oriented
//!    image. wgpu's texture origin is top-left; GL's framebuffer origin
//!    is bottom-left. Whether those cancel out across
//!    `texture_from_raw` is not something the type system can tell you,
//!    and getting it wrong renders every GPU canvas upside down.
//!
//! Each is checked below against a live GTK window on a real GPU.
//!
//! # Why it's an integration test, not a unit test
//!
//! GTK requires every call on the thread that ran `gtk_init`. The lib's
//! unit tests already consolidate into a single `#[test]` for that
//! reason (three GTK tests on three cargo threads SIGSEGV). An
//! integration test is a separate binary with its own process, so it
//! gets its own GTK main thread without fighting the lib's.
//!
//! Skips (rather than fails) when no display or no GL is available, so
//! it stays runnable on a headless CI box.

#![cfg(target_os = "linux")]

use backend_linux::gtk4;
use gtk4::prelude::*;
use runtime_core::primitives::graphics::{
    FramebufferOrigin, GlTarget, GraphicsTarget, OnReadyEvent,
};
use std::cell::RefCell;
use std::rc::Rc;

// --- GL constants used for the read-back path ----------------------------
const GL_FRAMEBUFFER: u32 = 0x8D40;
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
const GL_FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE: u32 = 0x8CD0;
const GL_FRAMEBUFFER_ATTACHMENT_OBJECT_NAME: u32 = 0x8CD1;
const GL_TEXTURE: i32 = 0x1702;
const GL_RENDERBUFFER: i32 = 0x8D41;
const GL_RGBA: u32 = 0x1908;
const GL_UNSIGNED_BYTE: u32 = 0x1401;

/// Physical size of the test area. Small but not square, so a
/// transposed (rather than merely flipped) result is also caught.
const W: u32 = 8;
const H: u32 = 4;

fn gl_fn<T: Copy>(target: &GlTarget, symbol: &str) -> Option<T> {
    let p = target.get_proc_address(symbol);
    if p.is_null() {
        return None;
    }
    Some(unsafe { *(&p as *const *const std::ffi::c_void as *const T) })
}

/// What the area's colour attachment is, as GL reports it. GTK is free
/// to back its framebuffer with either a texture or a renderbuffer, and
/// wgpu-hal needs a different constructor for each — so ask rather than
/// assume.
enum Attachment {
    Texture(u32),
    Renderbuffer(u32),
}

fn query_attachment(target: &GlTarget) -> Option<Attachment> {
    let get: unsafe extern "C" fn(u32, u32, u32, *mut i32) =
        gl_fn(target, "glGetFramebufferAttachmentParameteriv")?;
    let (mut kind, mut name) = (0i32, 0i32);
    unsafe {
        get(
            GL_FRAMEBUFFER,
            GL_COLOR_ATTACHMENT0,
            GL_FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE,
            &mut kind,
        );
        get(
            GL_FRAMEBUFFER,
            GL_COLOR_ATTACHMENT0,
            GL_FRAMEBUFFER_ATTACHMENT_OBJECT_NAME,
            &mut name,
        );
    }
    let name = u32::try_from(name).ok().filter(|n| *n != 0)?;
    match kind {
        GL_TEXTURE => Some(Attachment::Texture(name)),
        GL_RENDERBUFFER => Some(Attachment::Renderbuffer(name)),
        _ => None,
    }
}

/// Read the whole framebuffer back as RGBA8. Rows come out in GL order
/// — row 0 is the **bottom** row of the image.
fn read_pixels_bottom_up(target: &GlTarget) -> Option<Vec<u8>> {
    let read: unsafe extern "C" fn(i32, i32, i32, i32, u32, u32, *mut u8) =
        gl_fn(target, "glReadPixels")?;
    let finish: unsafe extern "C" fn() = gl_fn(target, "glFinish")?;
    let mut buf = vec![0u8; (W * H * 4) as usize];
    unsafe {
        finish();
        read(0, 0, W as i32, H as i32, GL_RGBA, GL_UNSIGNED_BYTE, buf.as_mut_ptr());
    }
    Some(buf)
}

/// The RGBA texel at (x, y) in **image** coordinates (y = 0 is the top
/// row), converting out of the bottom-up buffer `glReadPixels` produces.
fn texel(buf: &[u8], x: u32, y: u32) -> [u8; 4] {
    let gl_row = H - 1 - y;
    let i = ((gl_row * W + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

/// Adopt the lent GL context through wgpu's GLES HAL, exactly as a
/// consumer (canvas-vello, a wgpu host) would have to.
///
/// SAFETY contract of `gles::Adapter::new_external`: the context must be
/// current for the call, for every use of anything it returns, and for
/// every drop. The caller has just run `make_current()` and the whole
/// GPU lifetime is confined to this function, which returns before
/// yielding to the GLib main loop.
unsafe fn adopt(target: &GlTarget) -> Option<(wgpu::Device, wgpu::Queue, wgpu::Adapter)> {
    let exposed = unsafe {
        wgpu_hal::gles::Adapter::new_external(
            |sym| target.get_proc_address(sym),
            wgpu_types::GlBackendOptions::default(),
        )
    }?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::GL,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: Default::default(),
        backend_options: wgpu::BackendOptions::default(),
        display: None,
    });
    let adapter = unsafe { instance.create_adapter_from_hal(exposed) };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("gl-target-adoption"),
        required_features: wgpu::Features::empty(),
        required_limits: adapter.limits(),
        memory_hints: wgpu::MemoryHints::default(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        trace: wgpu::Trace::Off,
    }))
    .ok()?;
    Some((device, queue, adapter))
}

/// Wrap the area's colour attachment as a wgpu texture we can render
/// into. The `drop_callback` is the load-bearing bit: without it
/// wgpu-hal takes ownership and would delete GTK's attachment out from
/// under it on drop. A no-op callback means "borrowed, don't free".
unsafe fn wrap_attachment(
    device: &wgpu::Device,
    attachment: &Attachment,
) -> Option<wgpu::Texture> {
    let desc = wgpu::TextureDescriptor {
        label: Some("gtk-glarea-fbo"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
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
    let hal_texture = unsafe {
        let dev = device.as_hal::<wgpu_hal::api::Gles>()?;
        let name = match attachment {
            Attachment::Texture(n) | Attachment::Renderbuffer(n) => {
                std::num::NonZeroU32::new(*n)?
            }
        };
        match attachment {
            Attachment::Texture(_) => {
                dev.texture_from_raw(name, &hal_desc, Some(Box::new(|| {})))
            }
            Attachment::Renderbuffer(_) => {
                dev.texture_from_raw_renderbuffer(name, &hal_desc, Some(Box::new(|| {})))
            }
        }
    };
    Some(unsafe { device.create_texture_from_hal::<wgpu_hal::api::Gles>(hal_texture, &desc) })
}

/// Blit an asymmetric image, through wgpu, into the area's framebuffer.
///
/// The source is filled CPU-side in wgpu's row order (row 0 = top) with
/// the top half red and the bottom half black, then blitted with
/// `wgpu::util::TextureBlitter` — the same primitive the canvas
/// renderer uses to move its vello target onto a surface. A real draw
/// is required here: a render pass's `LoadOp::Clear` happens at
/// pass-begin, before any draw state applies, so `set_scissor_rect`
/// does NOT restrict it and a scissored clear paints the whole target.
fn blit_top_half(device: &wgpu::Device, queue: &wgpu::Queue, fbo_tex: &wgpu::Texture) {
    let src = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("orientation-source"),
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    let mut pixels = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            // Rows are top-down here — wgpu's texture origin is top-left.
            let red = y < H / 2;
            pixels[i] = if red { 255 } else { 0 };
            pixels[i + 3] = 255;
        }
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &src,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(W * 4),
            rows_per_image: Some(H),
        },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );

    let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());
    let dst_view = fbo_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let blitter = wgpu::util::TextureBlitter::new(device, wgpu::TextureFormat::Rgba8Unorm);
    let mut enc =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    blitter.copy(device, &mut enc, &src_view, &dst_view);
    queue.submit([enc.finish()]);
}

struct Outcome {
    loader_ok: bool,
    fbo: u32,
    adopted: bool,
    backend: Option<String>,
    /// Framebuffer-memory texels at the top-left and bottom-right, as
    /// `glReadPixels` sees them (see `texel`). Documents the raw layout
    /// relationship; what the user actually sees is `composited`.
    top_left: Option<[u8; 4]>,
    bottom_right: Option<[u8; 4]>,
    /// What the target claims about its row order.
    origin: Option<FramebufferOrigin>,
    /// `(top_is_red, bottom_is_red)` as GTK composites the area. Both
    /// halves are reported so "flipped" is distinguishable from
    /// "nothing rendered at all" — an all-black composite would
    /// otherwise masquerade as an upside-down one.
    composited: Option<(bool, bool)>,
    skip: Option<String>,
}

/// Ask GTK to composite the `GtkGLArea` and download the result, in
/// GTK's own (top-left origin) coordinate space. This is the only
/// measurement that answers "what does the user see" — `glReadPixels`
/// answers a different question (how the bytes sit in the framebuffer),
/// and the two differ by exactly the GL/wgpu origin convention.
///
/// `GdkTexture::download` writes `GDK_MEMORY_DEFAULT`, i.e. BGRA bytes,
/// so red is `[0, 0, 255, _]`.
fn gtk_composited(area: &gtk4::Widget) -> Option<(bool, bool)> {
    use gtk4::prelude::*;
    let paintable = gtk4::WidgetPaintable::new(Some(area));
    let snapshot = gtk4::Snapshot::new();
    paintable.snapshot(&snapshot, W as f64, H as f64);
    let node = snapshot.to_node()?;
    let renderer = area.native()?.renderer()?;
    let texture = renderer.render_texture(&node, None);

    let stride = (W * 4) as usize;
    let mut buf = vec![0u8; stride * H as usize];
    texture.download(&mut buf, stride);

    let is_red = |x: u32, y: u32| {
        let i = (y as usize) * stride + (x as usize) * 4;
        buf[i + 2] > 128 && buf[i] < 128
    };
    // Sample the middle of each half, away from any edge blending.
    Some((is_red(W / 2, 0), is_red(W / 2, H - 1)))
}

#[test]
fn gl_target_is_adoptable_by_wgpu_and_matches_its_declared_origin() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let outcome: Rc<RefCell<Option<Outcome>>> = Rc::new(RefCell::new(None));
    let out = outcome.clone();

    let on_ready = Box::new(move |ev: OnReadyEvent| {
        let GraphicsTarget::Gl(target) = &ev.target else {
            *out.borrow_mut() = Some(Outcome {
                loader_ok: false,
                fbo: 0,
                adopted: false,
                backend: None,
                top_left: None,
                bottom_right: None,
                origin: None,
            composited: None,
                skip: Some("backend handed back a RawWindow target, not Gl".into()),
            });
            return;
        };

        // (1) loader — a core entry point every consumer needs.
        let loader_ok = !target.get_proc_address("glBindFramebuffer").is_null();

        // (2) make_current must bind the AREA's fbo, not the default (0).
        target.make_current();
        let fbo = target.framebuffer();

        let mut o = Outcome {
            loader_ok,
            fbo,
            adopted: false,
            backend: None,
            top_left: None,
            bottom_right: None,
            origin: Some(target.origin()),
            composited: None,
            skip: None,
        };

        // (3) adopt + render + read back.
        let adopted = unsafe { adopt(target) };
        let Some((device, queue, adapter)) = adopted else {
            o.skip = Some("wgpu could not adopt the GL context".into());
            *out.borrow_mut() = Some(o);
            return;
        };
        o.adopted = true;
        o.backend = Some(format!("{:?}", adapter.get_info().backend));

        if let Some(att) = query_attachment(target) {
            if let Some(tex) = unsafe { wrap_attachment(&device, &att) } {
                blit_top_half(&device, &queue, &tex);
                // Rebind: wgpu's submit leaves ITS framebuffer bound, so
                // glReadPixels would otherwise sample the wrong target.
                target.make_current();
                if let Some(buf) = read_pixels_bottom_up(target) {
                    o.top_left = Some(texel(&buf, 0, 0));
                    o.bottom_right = Some(texel(&buf, W - 1, H - 1));
                }
            } else {
                o.skip = Some("could not wrap the GTK colour attachment".into());
            }
        } else {
            o.skip = Some("could not query the GTK colour attachment".into());
        }
        *out.borrow_mut() = Some(o);
    });

    let area = backend_linux::build_gl_area_for_test(
        on_ready,
        Box::new(|_| {}),
        Box::new(|| {}),
    );
    area.set_size_request(W as i32, H as i32);

    let window = gtk4::Window::new();
    window.set_default_size(W as i32, H as i32);
    window.set_child(Some(&area));
    window.present();

    // Pump the main loop until on_ready has fired (GTK realizes, gets a
    // GL context, and emits `render` asynchronously), with a bounded
    // number of iterations so a broken environment fails fast.
    let ctx = gtk4::glib::MainContext::default();
    for _ in 0..10_000 {
        if outcome.borrow().is_some() {
            break;
        }
        ctx.iteration(false);
    }

    let outcome = outcome.borrow_mut().take();
    let Some(o) = outcome else {
        eprintln!("SKIP: on_ready never fired (no GL context in this environment)");
        return;
    };

    // Composite through GTK from the test body, not from inside
    // `on_ready` — that runs during the `render` signal, and asking GTK
    // to re-render the same widget from within it is re-entrant.
    let mut o = o;
    if o.skip.is_none() {
        o.composited = gtk_composited(&area);
    }

    if let Some(reason) = &o.skip {
        if !o.adopted {
            eprintln!("SKIP: {reason}");
            return;
        }
        panic!("adopted the context but then failed: {reason}");
    }

    assert!(
        o.loader_ok,
        "GlTarget::get_proc_address could not resolve glBindFramebuffer — a \
         GPU library built on this target would have an empty function table",
    );
    assert_ne!(
        o.fbo, 0,
        "framebuffer() returned the DEFAULT framebuffer. make_current() must \
         leave the GLArea's own FBO bound (gtk_gl_area_attach_buffers), or the \
         author renders into GTK's toplevel buffer instead of the widget",
    );
    assert!(o.adopted, "wgpu could not adopt the lent GL context");
    assert_eq!(
        o.backend.as_deref(),
        Some("Gl"),
        "adoption must produce a GL-backed wgpu adapter, not a fresh native one",
    );

    // --- Orientation -----------------------------------------------------
    //
    // `blit_top_half` puts red in the TOP half of a source image
    // expressed in wgpu's (top-left origin) coordinates.
    //
    // Two different questions, deliberately measured separately:
    //
    // a) Where do the bytes land in the framebuffer? `glReadPixels`
    //    answers that, and its answer is "wgpu row N == GL row N" — no
    //    index flip happens across `texture_from_raw`. Since GL numbers
    //    rows from the BOTTOM, `texel(_, 0)` (the top row in image
    //    space) reads GL's last row, which is black.
    // b) What does the user see? Only GTK can answer that, because GTK
    //    is what composites the area's texture into the window — and it
    //    does so in its own top-left-origin space, applying the flip
    //    that GL textures conventionally need.
    //
    // (a) is what makes people assume the image is upside down; (b) is
    // the one that decides whether a consumer must pre-flip. They
    // disagree here, which is exactly why this test measures both
    // rather than reasoning from one.
    let tl = o.top_left.expect("read back top-left texel");
    let br = o.bottom_right.expect("read back bottom-right texel");
    assert_eq!(
        ([tl[0], tl[1], tl[2]], [br[0], br[1], br[2]]),
        ([0, 0, 0], [255, 0, 0]),
        "framebuffer memory layout changed: wgpu row N no longer maps to GL \
         row N across texture_from_raw (tl={tl:?} br={br:?}). Consumers' \
         orientation handling is derived from this — re-check it.",
    );

    let (top_red, bottom_red) = o
        .composited
        .expect("GTK could not composite the area for read-back");
    // Guard the conclusion: exactly one half must be red. If neither is,
    // nothing reached the framebuffer and any orientation verdict below
    // would be an artifact of reading an empty image.
    assert_ne!(
        (top_red, bottom_red),
        (false, false),
        "GTK composited an image with NO red half — the wgpu frame never \
         reached the GLArea framebuffer at all. Orientation is unknowable \
         from this; fix the render path first.",
    );
    assert_ne!(
        (top_red, bottom_red),
        (true, true),
        "both halves came back red — the source pattern did not survive \
         the blit, so orientation is unknowable from this",
    );
    // The target DECLARES its row order via `origin()`; this asserts the
    // declaration matches reality, so a consumer that folds `origin()`
    // into its base transform is provably correct rather than
    // hopefully correct. Red was blitted into the top half in wgpu's
    // top-left coordinates, so a `BottomLeft` target must show it at the
    // bottom, and a `TopLeft` target at the top.
    let expected_top_red = match o.origin.expect("target reported no origin") {
        FramebufferOrigin::TopLeft => true,
        FramebufferOrigin::BottomLeft => false,
    };
    assert_eq!(
        top_red, expected_top_red,
        "GlTarget::origin() reports {:?}, but GTK composited the frame the \
         other way up. Consumers fold origin() into their projection, so a \
         wrong declaration silently flips every GPU surface on this backend.",
        o.origin,
    );
}
