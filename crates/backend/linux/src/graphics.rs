//! `Element::Graphics` on the GTK4 backend — a `GtkGLArea` render surface.
//!
//! # The GTK4 constraint that shapes this whole module
//!
//! Every other backend implements `graphics` by handing the author a
//! `raw_window_handle::HasWindowHandle` (macOS: NSView+CAMetalLayer, iOS:
//! UIView+CAMetalLayer, Android: SurfaceView, web: `<canvas>`). The author's
//! GPU library then calls `wgpu::Instance::create_surface(&handle)` and drives
//! a real swapchain.
//!
//! **GTK4 cannot supply such a handle for a sub-widget.** Unlike GTK3, GTK4
//! removed per-widget native windows: the entire toplevel is one native
//! surface (one `wl_surface` on Wayland / one X11 window under XWayland) that
//! GTK renders every widget into with its own GSK renderer (Vulkan/GL/Cairo).
//! There is no per-widget `wl_surface`/XID to wrap in a `WaylandWindowHandle`/
//! `XlibWindowHandle`. The only raw handle obtainable is the *toplevel's*
//! surface (via `gdk4-wayland`/`gdk4-x11`), which (a) is the whole window, not
//! the graphics widget's rect, and (b) is already owned and continuously
//! presented by GTK's own renderer — a second wgpu swapchain configured on it
//! fights GTK for the buffer.
//!
//! So this backend takes GTK's sanctioned escape hatch, `GtkGLArea`: GTK hands
//! the widget a live `GdkGLContext` plus a framebuffer, and we lend both to the
//! author as a [`GlTarget`]. That variant of `GraphicsTarget` exists precisely
//! because the raw-window-handle model is unrepresentable here — see the
//! "Why the target is an enum" section on
//! `runtime_core::primitives::graphics`.
//!
//! # Rendering model
//!
//! GTK's `render` signal is the only moment GTK itself guarantees the context
//! is current and the area's framebuffer bound. But an author's render loop is
//! driven by *their* scheduler (a reactive effect, a `raf_loop`), which fires
//! at arbitrary points in the GLib main loop. Rather than force authors into a
//! draw-on-demand callback — which every other backend's contract does not have
//! — [`GlTarget::make_current`] lets them re-establish both at will:
//! `gtk_gl_area_make_current` + `gtk_gl_area_attach_buffers`, which GTK
//! documents for exactly this ("drawing into the GLArea outside the render
//! signal").
//!
//! After drawing, the author calls [`GlTarget::present`], which is
//! `queue_render` — GTK re-composites the area's framebuffer into the widget
//! tree on the next frame. The framebuffer's contents persist between frames
//! (GTK does not clear it; the `render` handler is expected to), so a frame
//! drawn outside the signal survives until the author draws the next one.
//! Our own `render` handler is what keeps it that way: before `on_ready` it
//! clears to [`CLEAR_RGBA`] so an un-adopted area isn't showing garbage, and
//! once the author has taken over it does nothing at all.
//!
//! Concretely: `make_current()` → draw with any GL/wgpu code → `present()`.

use gtk4::glib;
use gtk4::prelude::*;
use runtime_core::primitives::graphics::{
    FramebufferOrigin, GlContextProvider, GlTarget, GraphicsTarget, OnLost, OnReady, OnReadyEvent,
    OnResize, OnResizeEvent,
};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;

/// `GL_COLOR_BUFFER_BIT` — the framebuffer bit `glClear` clears.
const GL_COLOR_BUFFER_BIT: u32 = 0x0000_4000;
/// `glGetIntegerv` pname for the currently bound draw framebuffer.
const GL_FRAMEBUFFER_BINDING: u32 = 0x8CA6;

/// The color the GLArea clears its framebuffer to (idealyst indigo, sRGB
/// 0..1) *until* an author adopts it via `on_ready`. Visible proof the FBO is
/// real and we own the render pass; replaced wholesale once the author draws.
const CLEAR_RGBA: [f32; 4] = [0.106, 0.118, 0.216, 1.0];

/// Call a resolved GL entry point. `None` when the symbol is unavailable, in
/// which case the caller degrades rather than crashing.
fn gl_fn<T: Copy>(symbol: &str) -> Option<T> {
    let p = crate::gl_loader::proc_address(symbol);
    if p.is_null() {
        return None;
    }
    // SAFETY: the loader returns a GL entry point for `symbol`; `T` is the
    // matching `extern "C" fn` signature at each (single) call site below.
    debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*const c_void>());
    Some(unsafe { *(&p as *const *const c_void as *const T) })
}

/// Clear the currently bound framebuffer to `rgba`. No-op if the entry points
/// can't be resolved (GTK still shows the widget, just uncleared).
fn clear_to(rgba: [f32; 4]) {
    let (Some(clear_color), Some(clear)) = (
        gl_fn::<unsafe extern "C" fn(f32, f32, f32, f32)>("glClearColor"),
        gl_fn::<unsafe extern "C" fn(u32)>("glClear"),
    ) else {
        return;
    };
    let [r, g, b, a] = rgba;
    // SAFETY: only reached from the `render` signal / with the area's context
    // made current, which is what makes these calls legal.
    unsafe {
        clear_color(r, g, b, a);
        clear(GL_COLOR_BUFFER_BIT);
    }
}

/// The GL context + framebuffer of one `GtkGLArea`, lent to the author.
///
/// Holds the widget itself rather than a snapshotted context/FBO id: GTK
/// reallocates both across a resize or a re-realize, so every question has to
/// be re-asked of the live widget. Caching either would hand the author a
/// destroyed context after the first resize.
struct GtkGlProvider {
    area: gtk4::GLArea,
    /// The author's per-frame draw, invoked from the `render` signal (see
    /// `on_render`). Shared with the signal handler so a `present()`
    /// (queue_render) causes GTK to emit `render`, which runs this WITH the
    /// area's FBO bound and its context current — the only place GtkGLArea
    /// reliably composites what's drawn. Drawing out-of-band via
    /// `make_current` and hoping GTK re-composites the FBO does NOT work
    /// (the widget shows the last frame GTK actually captured — the
    /// "canvas stuck on the clear colour" bug).
    render_cb: Rc<RefCell<Option<Box<dyn FnMut()>>>>,
}

impl GlContextProvider for GtkGlProvider {
    fn get_proc_address(&self, symbol: &str) -> *const c_void {
        crate::gl_loader::proc_address(symbol)
    }

    fn make_current(&self) {
        // `make_current` alone binds the context but leaves the DEFAULT
        // framebuffer bound — drawing then goes to GTK's own toplevel buffer
        // (or nowhere), not into the area. `attach_buffers` is the documented
        // pairing for drawing outside the `render` signal: it makes the
        // context current AND binds the area's FBO + its buffers, allocating
        // them if a resize invalidated the old ones.
        self.area.make_current();
        self.area.attach_buffers();
    }

    fn framebuffer(&self) -> u32 {
        // Ask GL what `attach_buffers` just bound rather than reaching into
        // GTK's private `GtkGLArea` struct for its FBO id. `attach_buffers`
        // is the only public API that surfaces it, and it does so only via
        // the binding — so this must be read AFTER `make_current`.
        let Some(get_integerv) = gl_fn::<unsafe extern "C" fn(u32, *mut i32)>("glGetIntegerv")
        else {
            return 0;
        };
        let mut fbo: i32 = 0;
        // SAFETY: `make_current` precedes this per the `GlTarget` contract.
        unsafe { get_integerv(GL_FRAMEBUFFER_BINDING, &mut fbo) };
        fbo.max(0) as u32
    }

    fn origin(&self) -> FramebufferOrigin {
        // GL's framebuffer origin. Measured, not assumed: wrapping this
        // FBO's colour attachment through wgpu's `texture_from_raw`
        // maps wgpu row N to GL row N (no index flip), and GDK then
        // composites the GL texture bottom-up — so a top-left-origin
        // renderer lands on screen upside down unless it flips.
        // `tests/gl_target_adoption.rs` measures both halves of that.
        FramebufferOrigin::BottomLeft
    }

    fn set_render_callback(&self, cb: Box<dyn FnMut()>) {
        *self.render_cb.borrow_mut() = Some(cb);
    }

    fn present(&self) {
        // Schedule a `render` emission. The actual draw happens in that
        // signal (via `render_cb`), because GTK only composites what is
        // drawn during its own render pass — see `render_cb`'s doc.
        self.area.queue_render();
    }
}

/// Whether a `resize` signal reports a usable, changed size. Mirrors the
/// macOS `resize_decision`: skip degenerate (≤1px, mid-realize) sizes and
/// dedupe unchanged repeats so `on_resize` only fires on real changes.
fn resize_decision(last: (u32, u32), new: (u32, u32)) -> Option<(u32, u32)> {
    let (w, h) = new;
    if w <= 1 || h <= 1 || new == last {
        return None;
    }
    Some(new)
}

/// Whether the first `render` tick can fire `on_ready` yet: only once the
/// framebuffer has a real size. GTK emits `render` during realize with a
/// degenerate 1×N / N×1 area, and an author that sized a swapchain or a
/// vello target off that would allocate a garbage-sized buffer and never be
/// told otherwise (`on_resize` only reports *changes*).
fn ready_decision(fired: bool, size: (u32, u32)) -> Option<(u32, u32)> {
    if fired || size.0 <= 1 || size.1 <= 1 {
        return None;
    }
    Some(size)
}

/// Build a `GtkGLArea` and wire the graphics lifecycle callbacks to its GL
/// signals. Returned as a `gtk4::Widget` for the backend to wrap into a layout
/// node.
pub(crate) fn build_gl_area(
    on_ready: OnReady,
    on_resize: OnResize,
    on_lost: OnLost,
) -> gtk4::Widget {
    let area = gtk4::GLArea::new();
    area.set_auto_render(true);
    area.set_has_depth_buffer(false);
    area.set_has_stencil_buffer(false);
    // Prefer desktop GL; fall back to GLES if that's all the driver offers.
    area.set_required_version(3, 0);

    // --- render: fire on_ready once, then run the author's draw IN-signal ---
    let on_ready_cell: Rc<RefCell<Option<OnReady>>> = Rc::new(RefCell::new(Some(on_ready)));
    let adopted = Rc::new(Cell::new(false));
    // The author's per-frame draw, shared between the provider (which stores
    // it via `on_render`) and this signal handler (which runs it). This is
    // the fix for the "canvas stuck on the clear colour" bug: the draw MUST
    // happen inside the `render` signal — GTK only composites what is drawn
    // during its own render pass, so an out-of-band blit is never shown.
    let render_cb: Rc<RefCell<Option<Box<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    {
        let on_ready_cell = on_ready_cell.clone();
        let adopted = adopted.clone();
        let render_cb = render_cb.clone();
        area.connect_render(move |area, _ctx| {
            // GTK has made the context current and bound the area's FBO
            // before emitting `render`.
            if !adopted.get() {
                // Pre-adoption: clear to a placeholder so an un-adopted area
                // isn't garbage, and fire `on_ready` on the first real size.
                // `on_ready` is where the author registers `render_cb` (via
                // `on_render`), so it must run BEFORE the draw below.
                clear_to(CLEAR_RGBA);
                let size = (
                    area.width().max(0) as u32 * area.scale_factor().max(1) as u32,
                    area.height().max(0) as u32 * area.scale_factor().max(1) as u32,
                );
                let fired = on_ready_cell.borrow().is_none();
                if let Some(size) = ready_decision(fired, size) {
                    // Take the callback out before invoking: author code can
                    // re-enter GTK (queue_render → render), and a live
                    // `borrow_mut` across that would panic.
                    let cb = on_ready_cell.borrow_mut().take();
                    if let Some(mut cb) = cb {
                        adopted.set(true);
                        let target = GlTarget::new(Rc::new(GtkGlProvider {
                            area: area.clone(),
                            render_cb: render_cb.clone(),
                        }));
                        cb(OnReadyEvent {
                            target: GraphicsTarget::Gl(target),
                            size,
                            // `size` is already physical (logical × scale
                            // factor), matching the web backend's "size is
                            // physical, no separate scale" contract.
                            scale: 1.0,
                        });
                        // Keep the callback: `on_lost` (unrealize) can be
                        // followed by a fresh realize, and the contract
                        // promises a new `on_ready` when the context returns.
                        *on_ready_cell.borrow_mut() = Some(cb);
                    }
                }
            }

            // Run the author's draw IN this render pass — the ONLY place
            // GtkGLArea composites what's drawn. This runs on the adoption
            // pass too (right after `on_ready` registered it), so the first
            // frame shows author content instead of the placeholder clear;
            // and on every subsequent `present()`-triggered pass. Take it
            // out across the call: author code may re-enter GTK (a nested
            // queue_render) and a live borrow would panic.
            if adopted.get() {
                let cb = render_cb.borrow_mut().take();
                if let Some(mut cb) = cb {
                    cb();
                    let mut slot = render_cb.borrow_mut();
                    if slot.is_none() {
                        *slot = Some(cb);
                    }
                }
            }
            glib::Propagation::Stop
        });
    }

    // --- resize: report real, changed physical sizes to on_resize ----------
    // GLArea's `resize` passes the framebuffer size in physical pixels
    // (logical × scale-factor) — exactly what the graphics contract wants.
    let on_resize_cell: Rc<RefCell<OnResize>> = Rc::new(RefCell::new(on_resize));
    let last_size = Rc::new(Cell::new((0u32, 0u32)));
    {
        let on_resize_cell = on_resize_cell.clone();
        let last_size = last_size.clone();
        let adopted = adopted.clone();
        area.connect_resize(move |area, w, h| {
            let new = (w.max(0) as u32, h.max(0) as u32);
            let Some(new) = resize_decision(last_size.get(), new) else {
                return;
            };
            last_size.set(new);
            // Per the graphics contract, `on_resize` fires only AFTER
            // `on_ready` has seeded the target.
            if !adopted.get() {
                return;
            }
            let scale = area.scale_factor().max(1) as f32;
            (on_resize_cell.borrow_mut())(OnResizeEvent { size: new, scale });
        });
    }

    // --- unrealize: the GL context is being torn down → surface lost -------
    let on_lost_cell: Rc<RefCell<Option<OnLost>>> = Rc::new(RefCell::new(Some(on_lost)));
    {
        let on_lost_cell = on_lost_cell.clone();
        let adopted = adopted.clone();
        area.connect_unrealize(move |_area| {
            if !adopted.replace(false) {
                return;
            }
            // Re-arm: a realize after this must fire `on_ready` again with the
            // new context. `on_ready_cell` still holds the callback.
            last_size.set((0, 0));
            if let Some(cb) = on_lost_cell.borrow_mut().as_mut() {
                cb();
            }
        });
    }

    area.upcast::<gtk4::Widget>()
}

#[cfg(test)]
mod tests {
    use super::{ready_decision, resize_decision};

    #[test]
    fn resize_decision_reports_real_change() {
        assert_eq!(resize_decision((400, 300), (800, 600)), Some((800, 600)));
    }

    #[test]
    fn resize_decision_dedupes_unchanged() {
        assert_eq!(resize_decision((800, 600), (800, 600)), None);
    }

    #[test]
    fn resize_decision_skips_degenerate() {
        // Mid-realize / not-yet-sized framebuffers report ≤1px — skip them so
        // on_resize never sees a bogus 1×N surface.
        assert_eq!(resize_decision((800, 600), (1, 600)), None);
        assert_eq!(resize_decision((800, 600), (800, 0)), None);
    }

    #[test]
    fn ready_decision_fires_once_on_a_real_size() {
        assert_eq!(ready_decision(false, (800, 600)), Some((800, 600)));
        // Already fired — the author gets exactly one on_ready per context.
        assert_eq!(ready_decision(true, (800, 600)), None);
    }

    /// GTK emits `render` during realize with a degenerate area. Firing
    /// `on_ready` there hands the author a 1×N size to allocate against, and
    /// since `on_resize` only reports *changes* they'd never be corrected.
    #[test]
    fn ready_decision_waits_for_a_real_framebuffer_size() {
        assert_eq!(ready_decision(false, (0, 0)), None);
        assert_eq!(ready_decision(false, (1, 600)), None);
        assert_eq!(ready_decision(false, (800, 1)), None);
    }
}
