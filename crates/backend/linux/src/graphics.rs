//! `Element::Graphics` on the GTK4 backend — a `GtkGLArea` render surface.
//!
//! # The GTK4 constraint that shapes this whole module
//!
//! Every other backend implements `graphics` by handing the author a
//! `raw_window_handle::HasWindowHandle` (macOS: NSView+CAMetalLayer, iOS:
//! UIView+CAMetalLayer, Android: SurfaceView, web: `<canvas>`). The author's
//! GPU library (`canvas`/`pdf` via wgpu) then calls
//! `wgpu::Instance::create_surface(&handle)` and drives a real swapchain —
//! see `crates/sdk/client/canvas/vello/src/render.rs::RenderState::new`, which
//! does exactly `instance.create_surface(surface_target)` +
//! `surface.configure(..)` + `surface.get_current_texture()`.
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
//! fights GTK for the buffer. So the raw-window-handle contract is a dead end
//! for embedded GPU content on GTK4.
//!
//! GTK4's *sanctioned* ways to embed externally-rendered GPU content are:
//!
//! 1. **`GtkGLArea`** — GTK hands the widget a live `GdkGLContext` + a
//!    framebuffer (FBO) bound during the `render` signal. This is a GL
//!    *context*, not a window handle. wgpu can drive it only via its GLES HAL
//!    (`wgpu::hal::gles::Adapter` built from a `glow`/`epoxy` proc-address
//!    loader, rendering into the bound FBO) — NOT via `create_surface`.
//! 2. **`GtkGraphicsOffload` + `GdkDmabufTexture`** (GTK ≥ 4.14) — render
//!    offscreen (headless wgpu → texture), export the texture as a dma-buf,
//!    import it as a `GdkDmabufTexture`, and display it in a `GtkPicture`
//!    inside a `GtkGraphicsOffload` so the compositor can promote it to a
//!    hardware overlay. This is the *most correct* sub-widget path, but again
//!    wgpu renders headless — no swapchain surface.
//!
//! **Both idiomatic GTK4 paths produce a GL context / render target, not a
//! `create_surface`-compatible `HasWindowHandle`.** So `create_graphics`'
//! current contract — "call `on_ready` with a `GraphicsSurface` the author
//! turns into a wgpu `Surface`" — cannot be honored on GTK4 as written. See
//! the module tail for the framework change that unblocks it.
//!
//! # What this module ships
//!
//! Path 1's foundation, proven end-to-end: a real `GtkGLArea` mounted in the
//! layout tree that acquires a GL context and **clears its FBO to a color**
//! from the `render` signal — concrete evidence the widget + GL context + FBO
//! wiring is live on this host (Wayland, RADV). `on_resize`/`on_lost` are wired
//! to the GLArea's `resize`/`unrealize` signals. `on_ready` is deliberately
//! *not* fired: it demands a `GraphicsSurface` (raw-window-handle) this backend
//! provably cannot construct (above). Firing it with a fabricated or toplevel
//! handle would either error in `create_surface` or hijack the whole window —
//! neither is honest embedded rendering, so we don't. The callback is retained
//! at the exact call site the framework fix plugs into.

use gtk4::glib;
use gtk4::prelude::*;
use runtime_core::primitives::graphics::{OnLost, OnReady, OnResize, OnResizeEvent};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

/// `GL_COLOR_BUFFER_BIT` — the framebuffer bit `glClear` clears.
const GL_COLOR_BUFFER_BIT: u32 = 0x0000_4000;

/// The color the GLArea clears its framebuffer to (idealyst indigo, sRGB
/// 0..1). Visible proof the FBO is real and we own the render pass.
const CLEAR_RGBA: [f32; 4] = [0.106, 0.118, 0.216, 1.0];

/// GL entry points read from the already-in-process libepoxy. GTK links
/// libepoxy for its own GL renderer, so `dlopen("libepoxy.so.0", RTLD_LAZY)`
/// just bumps the refcount on the loaded image. libepoxy does NOT export
/// `glClearColor` as a function symbol — it exports `epoxy_glClearColor`, a
/// DATA symbol holding a lazily-resolved dispatch pointer (initially a
/// resolver thunk that, on first call, looks up the real GL entry via the
/// *current* `GdkGLContext` and rewrites itself). So we `dlsym` the variable's
/// address and read the stored function pointer out of it. Calling it is sound
/// only while a GL context is current — which GTK guarantees before it emits
/// the `render` signal.
struct GlFns {
    clear_color: unsafe extern "C" fn(f32, f32, f32, f32),
    clear: unsafe extern "C" fn(u32),
}

// GTK is single-threaded, but `OnceLock` requires `Sync`; raw `fn` pointers are
// `Send + Sync`, so this is sound.
unsafe impl Sync for GlFns {}
unsafe impl Send for GlFns {}

static GL_FNS: OnceLock<Option<GlFns>> = OnceLock::new();

/// Read `epoxy_glClearColor` / `epoxy_glClear`'s dispatch pointers from
/// libepoxy once. `None` if the library or a symbol can't be found (then
/// `render` no-ops rather than crashing — GTK still shows the widget, just
/// uncleared).
fn gl_fns() -> Option<&'static GlFns> {
    GL_FNS
        .get_or_init(|| unsafe {
            // RTLD_LAZY, no NOLOAD: GTK has already mapped libepoxy, so this
            // reuses it (dlopen refcounts by soname). We never dlclose — the
            // handle lives for the process, keeping the dispatch pointers valid.
            let mut handle = libc::dlopen(c"libepoxy.so.0".as_ptr(), libc::RTLD_LAZY);
            if handle.is_null() {
                handle = libc::dlopen(c"libepoxy.so".as_ptr(), libc::RTLD_LAZY);
            }
            if handle.is_null() {
                return None;
            }
            // `dlsym` on a data symbol returns the ADDRESS OF the
            // `epoxy_glClearColor` variable; the fn pointer is the value stored
            // there, so we deref once.
            let read = |name: &std::ffi::CStr| -> *const std::ffi::c_void {
                let addr = libc::dlsym(handle, name.as_ptr());
                if addr.is_null() {
                    std::ptr::null()
                } else {
                    *(addr as *const *const std::ffi::c_void)
                }
            };
            let cc = read(c"epoxy_glClearColor");
            let cl = read(c"epoxy_glClear");
            if cc.is_null() || cl.is_null() {
                return None;
            }
            Some(GlFns {
                clear_color: std::mem::transmute::<
                    *const std::ffi::c_void,
                    unsafe extern "C" fn(f32, f32, f32, f32),
                >(cc),
                clear: std::mem::transmute::<
                    *const std::ffi::c_void,
                    unsafe extern "C" fn(u32),
                >(cl),
            })
        })
        .as_ref()
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

/// Build a `GtkGLArea` that clears to [`CLEAR_RGBA`] every frame and wires the
/// graphics lifecycle callbacks to the GLArea's GL signals. Returned as a
/// `gtk4::Widget` for the backend to wrap into a layout node.
pub(crate) fn build_gl_area(
    on_ready: OnReady,
    on_resize: OnResize,
    on_lost: OnLost,
) -> gtk4::Widget {
    let area = gtk4::GLArea::new();
    // Drive rendering ourselves from the `render` signal every frame.
    area.set_auto_render(true);
    area.set_has_depth_buffer(false);
    area.set_has_stencil_buffer(false);
    // Prefer desktop GL; fall back to GLES if that's all the driver offers.
    area.set_required_version(3, 0);

    // --- render: clear the FBO to a color (proves context + FBO are live) ---
    area.connect_render(|_area, _ctx| {
        // GTK has bound the GLArea's framebuffer and made its context current
        // before emitting `render`, so these calls land on the widget's FBO.
        if let Some(f) = gl_fns() {
            let [r, g, b, a] = CLEAR_RGBA;
            unsafe {
                (f.clear_color)(r, g, b, a);
                (f.clear)(GL_COLOR_BUFFER_BIT);
            }
        }
        // Stop emission: we are the renderer for this area.
        glib::Propagation::Stop
    });

    // --- resize: report real, changed physical sizes to on_resize ----------
    // GLArea's `resize` passes the framebuffer size in physical pixels
    // (logical × scale-factor) — exactly what the graphics contract wants.
    let on_resize_cell: Rc<RefCell<OnResize>> = Rc::new(RefCell::new(on_resize));
    let last_size = Rc::new(std::cell::Cell::new((0u32, 0u32)));
    {
        let on_resize_cell = on_resize_cell.clone();
        let last_size = last_size.clone();
        area.connect_resize(move |area, w, h| {
            let new = (w.max(0) as u32, h.max(0) as u32);
            let Some(new) = resize_decision(last_size.get(), new) else {
                return;
            };
            last_size.set(new);
            let scale = area.scale_factor().max(1) as f32;
            // Per the graphics contract, `on_resize` fires only AFTER
            // `on_ready` has seeded the surface. Since `on_ready` cannot fire
            // on GTK4 (see module docs), this is currently inert — but it is
            // wired to the correct signal so it works the moment the framework
            // exposes a GL-context render target and `on_ready` can fire.
            (on_resize_cell.borrow_mut())(OnResizeEvent { size: new, scale });
        });
    }

    // --- unrealize: the GL context is being torn down → surface lost -------
    let on_lost_cell: Rc<RefCell<Option<OnLost>>> = Rc::new(RefCell::new(Some(on_lost)));
    {
        let on_lost_cell = on_lost_cell.clone();
        area.connect_unrealize(move |_area| {
            if let Some(cb) = on_lost_cell.borrow_mut().as_mut() {
                cb();
            }
        });
    }

    // --- on_ready: BLOCKED on the raw-window-handle contract ----------------
    // We cannot construct a `GraphicsSurface` (HasWindowHandle) for a
    // `GtkGLArea` — it exposes a `GdkGLContext` + FBO, not a window handle
    // wgpu's `create_surface` accepts (see module docs). Retained here, at the
    // exact seam the framework fix plugs into: once `OnReadyEvent` can carry a
    // GL-context render target instead of only a raw-window-handle, this fires
    // from `connect_render`'s first invocation with the live context + FBO id +
    // physical size. Until then it stays un-fired rather than handing back a
    // fabricated or whole-window handle.
    let _on_ready_blocked: Rc<RefCell<Option<OnReady>>> = Rc::new(RefCell::new(Some(on_ready)));

    area.upcast::<gtk4::Widget>()
}

#[cfg(test)]
mod tests {
    use super::resize_decision;

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
}
