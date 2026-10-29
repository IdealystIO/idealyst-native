//! A `GpuSurface` built straight from a GTK4 `graphics` target — no
//! `gpu_surface::mount` — must put its frame on screen.
//!
//! # The bug this guards against
//!
//! GtkGLArea only composites what is drawn inside its own `render` signal, so
//! the GL present is a blit registered through `GlTarget::on_render`. That
//! registration used to live in `mount`, not in `GpuSurface`: an app driving a
//! plain `graphics` primitive (its own render loop, `GpuSurface::new` in
//! `on_ready`) adopted the context, rendered, called `finish_present` — and
//! GTK kept compositing the backend's placeholder clear forever, because no
//! blit was ever registered. `GpuSurface` now registers it on construction.
//!
//! # What this asserts
//!
//! The frame target is filled RED over BLUE (top half / bottom half) and
//! presented with `begin_present` / `finish_present`. GTK then composites the
//! widget and the read-back must show red on top and blue below — which also
//! pins the bottom-left-origin flip on this path.
//!
//! Skips (not fails) with no display / no GL, so it stays CI-safe.

#![cfg(target_os = "linux")]

use backend_linux::gtk4;
use gpu_surface::{FrameAlpha, GpuSurface, Requirements};
use gtk4::prelude::*;
use runtime_shared::primitives::graphics::{GraphicsTarget, OnReadyEvent};
use std::cell::RefCell;
use std::rc::Rc;

const W: u32 = 8;
const H: u32 = 8;
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// Composite the widget through GTK and return its pixels as RGBA rows.
/// `GdkTexture::download` writes `GDK_MEMORY_DEFAULT` (BGRA premultiplied).
fn gtk_composite(area: &gtk4::Widget) -> Option<Vec<[u8; 4]>> {
    let paintable = gtk4::WidgetPaintable::new(Some(area));
    let snapshot = gtk4::Snapshot::new();
    paintable.snapshot(&snapshot, W as f64, H as f64);
    let node = snapshot.to_node()?;
    let renderer = area.native()?.renderer()?;
    let texture = renderer.render_texture(&node, None);
    let stride = (W * 4) as usize;
    let mut buf = vec![0u8; stride * H as usize];
    texture.download(&mut buf, stride);
    Some(buf.chunks_exact(4).map(|p| [p[2], p[1], p[0], p[3]]).collect())
}

fn px(img: &[[u8; 4]], x: u32, y: u32) -> [u8; 4] {
    img[(y * W + x) as usize]
}

fn is(c: [u8; 4], want: [u8; 4]) -> bool {
    c.iter().zip(want).all(|(a, b)| a.abs_diff(b) < 40)
}

/// Fill the frame target: `RED` in the top half, `BLUE` below.
fn paint(gpu: &GpuSurface) {
    let (w, h) = gpu.size();
    let mut bytes = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for _ in 0..w {
            bytes.extend_from_slice(if y < h / 2 { &RED } else { &BLUE });
        }
    }
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: gpu.target(),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    let frame = gpu.begin_present(&mut encoder).expect("the GL path always yields a frame");
    gpu.queue.submit([encoder.finish()]);
    gpu.finish_present(frame);
}

#[test]
fn regression_gpu_surface_outside_mount_presents_on_gtk() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    // Owned by the "app", exactly like a hand-rolled renderer would hold it.
    let held: Rc<RefCell<Option<GpuSurface>>> = Rc::new(RefCell::new(None));
    let adopted = Rc::new(std::cell::Cell::new(false));
    let on_ready = {
        let (held, adopted) = (held.clone(), adopted.clone());
        Box::new(move |ev: OnReadyEvent| {
            if !matches!(ev.target, GraphicsTarget::Gl(_)) {
                return;
            }
            let Some(gpu) = GpuSurface::new(
                ev.target,
                ev.size,
                ev.scale,
                Requirements::NONE,
                FrameAlpha::Premultiplied,
                "gpu-surface-test",
            ) else {
                return;
            };
            adopted.set(true);
            paint(&gpu);
            *held.borrow_mut() = Some(gpu);
        })
    };

    let area = backend_linux::build_gl_area_for_test(on_ready, Box::new(|_| {}), Box::new(|| {}));
    area.set_size_request(W as i32, H as i32);
    let window = gtk4::Window::new();
    window.set_default_size(W as i32, H as i32);
    window.set_child(Some(&area));
    window.present();

    let shows_frame = |img: &[[u8; 4]]| is(px(img, W / 2, 1), RED) && is(px(img, W / 2, H - 2), BLUE);
    let ctx = gtk4::glib::MainContext::default();
    for _ in 0..20_000 {
        ctx.iteration(false);
        if adopted.get() && gtk_composite(&area).is_some_and(|img| shows_frame(&img)) {
            break;
        }
    }

    if !adopted.get() {
        eprintln!("SKIP: the GL context was never adopted (no GL in this environment)");
        return;
    }
    let Some(img) = gtk_composite(&area) else {
        eprintln!("SKIP: could not composite the widget for read-back");
        return;
    };
    let (top, bottom) = (px(&img, W / 2, 1), px(&img, W / 2, H - 2));
    assert!(
        shows_frame(&img),
        "GTK composited top {top:?} / bottom {bottom:?}, want red over blue. A GpuSurface \
         presented outside `mount` must register its own in-signal blit (and flip for the \
         bottom-left GL origin).",
    );

    // Drop with the context current, as the GpuSurface contract requires.
    held.borrow_mut().take();
}
