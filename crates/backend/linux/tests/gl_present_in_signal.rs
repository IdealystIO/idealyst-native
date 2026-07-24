//! Proves the fix for the "canvas stuck on the clear colour" bug: a draw
//! registered via `GlTarget::on_render` and triggered by `present()` is
//! actually composited by GTK.
//!
//! # The bug this guards against
//!
//! The first GL present model drew *out-of-band* — the author called
//! `make_current`, blitted into the FBO, then `present()` (queue_render)
//! with a render-signal handler that did nothing. GtkGLArea does NOT
//! reliably composite an out-of-band draw: the widget kept showing the
//! single pre-adoption clear frame (idealyst indigo), so the whiteboard
//! canvas came up a static dark blue and never updated.
//!
//! The fix moves the draw INTO GTK's `render` signal: the author
//! registers it via `on_render`, and the backend's render handler runs
//! it with the context current and the area's FBO bound — the only place
//! GtkGLArea guarantees the result is composited.
//!
//! # What this test asserts
//!
//! It registers an `on_render` that clears the framebuffer to a known
//! GREEN, triggers a `present()`, pumps the loop, then asks GTK to
//! composite the widget and reads the pixels back. Green means the
//! in-signal draw reached the screen. If this ever regresses to the
//! out-of-band model, the composite would come back the indigo clear
//! colour (or unchanged) and this fails.
//!
//! Skips (not fails) with no display / no GL, so it stays CI-safe.

#![cfg(target_os = "linux")]

use backend_linux::gtk4;
use gtk4::prelude::*;
use runtime_core::primitives::graphics::{GraphicsTarget, OnReadyEvent};
use std::cell::Cell;
use std::rc::Rc;

const GL_COLOR_BUFFER_BIT: u32 = 0x0000_4000;
const W: u32 = 8;
const H: u32 = 6;

/// Resolve a GL entry point through the target's own loader.
fn gl_fn<T: Copy>(target: &runtime_core::primitives::graphics::GlTarget, sym: &str) -> Option<T> {
    let p = target.get_proc_address(sym);
    if p.is_null() {
        return None;
    }
    Some(unsafe { *(&p as *const *const std::ffi::c_void as *const T) })
}

/// Composite the widget through GTK and report whether the centre pixel
/// is GREEN. `GdkTexture::download` writes `GDK_MEMORY_DEFAULT` (BGRA), so
/// green is `[_, >128, _, _]` with low red/blue.
fn gtk_shows_green(area: &gtk4::Widget) -> Option<bool> {
    let paintable = gtk4::WidgetPaintable::new(Some(area));
    let snapshot = gtk4::Snapshot::new();
    paintable.snapshot(&snapshot, W as f64, H as f64);
    let node = snapshot.to_node()?;
    let renderer = area.native()?.renderer()?;
    let texture = renderer.render_texture(&node, None);
    let stride = (W * 4) as usize;
    let mut buf = vec![0u8; stride * H as usize];
    texture.download(&mut buf, stride);
    let i = (H as usize / 2) * stride + (W as usize / 2) * 4;
    // BGRA: green dominant, red+blue low.
    Some(buf[i + 1] > 128 && buf[i] < 128 && buf[i + 2] < 128)
}

#[test]
fn on_render_draw_is_composited_by_gtk() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }

    let fired = Rc::new(Cell::new(false));
    let f = fired.clone();

    let on_ready = Box::new(move |ev: OnReadyEvent| {
        let GraphicsTarget::Gl(gl) = ev.target else {
            return;
        };
        f.set(true);
        // Register the in-signal draw: clear the bound FBO to green.
        let gl_for_draw = gl.clone();
        gl.on_render(Box::new(move || {
            let (Some(clear_color), Some(clear)) = (
                gl_fn::<unsafe extern "C" fn(f32, f32, f32, f32)>(&gl_for_draw, "glClearColor"),
                gl_fn::<unsafe extern "C" fn(u32)>(&gl_for_draw, "glClear"),
            ) else {
                return;
            };
            // SAFETY: on_render runs with the area's context current + FBO
            // bound, which is exactly what makes these GL calls legal.
            unsafe {
                clear_color(0.0, 1.0, 0.0, 1.0);
                clear(GL_COLOR_BUFFER_BIT);
            }
        }));
        // Schedule the render pass that runs the callback above.
        gl.present();
    });

    let area = backend_linux::build_gl_area_for_test(on_ready, Box::new(|_| {}), Box::new(|| {}));
    area.set_size_request(W as i32, H as i32);

    let window = gtk4::Window::new();
    window.set_default_size(W as i32, H as i32);
    window.set_child(Some(&area));
    window.present();

    let ctx = gtk4::glib::MainContext::default();
    // Pump long enough for realize → on_ready → the follow-up present's
    // render pass to run the callback.
    for _ in 0..20_000 {
        ctx.iteration(false);
        if fired.get() && gtk_shows_green(&area) == Some(true) {
            break;
        }
    }

    if !fired.get() {
        eprintln!("SKIP: on_ready never fired (no GL context in this environment)");
        return;
    }

    match gtk_shows_green(&area) {
        Some(true) => { /* the in-signal draw was composited — fix verified */ }
        Some(false) => panic!(
            "on_render's green draw was NOT composited — GTK is showing something \
             else (the clear colour / stale frame). The in-signal present model is \
             broken; a draw registered via on_render + present() must reach the screen.",
        ),
        None => eprintln!("SKIP: could not composite the widget for read-back"),
    }
}
