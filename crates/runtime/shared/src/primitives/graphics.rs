//! Graphics primitive — a backend-provided platform surface the
//! author renders to with their own GPU library.
//!
//! The framework's job is narrow on purpose: stand up a drawable
//! surface in the layout (`<canvas>` on web, `SurfaceView` on
//! Android, `UIView` + `CAMetalLayer` on iOS, `GtkGLArea` on Linux),
//! expose it as a [`GraphicsTarget`] — usually a standard
//! [`raw_window_handle`] handle, a lent GL context where the toolkit
//! can't provide one — and notify the author when it's ready /
//! resized / lost. Everything past that — picking a GPU
//! backend, building a render loop, allocating resources — is the
//! author's call. Most authors will pair this with `wgpu`, which
//! takes any `HasWindowHandle + HasDisplayHandle` and dispatches to
//! the platform-native API (Metal on iOS/macOS, Vulkan on
//! Android/Linux/Windows, WebGPU/WebGL2 on web). But softbuffer,
//! glow, vello, raqote, etc. all also accept the same handle traits.
//!
//! # Why surface-provider, not GPU-provider?
//!
//! An earlier shape of this primitive baked `wgpu` into the
//! framework: the backend ran `Instance::create_surface +
//! request_adapter + request_device` and handed the user a typed
//! GPU context. That coupled every backend (web/iOS/Android) to
//! wgpu, which made cross-platform support painful — each backend
//! had to reimplement the wgpu init dance and serialize wgpu types
//! through `Rc<dyn Any>` to keep runtime-core wgpu-free. The new
//! shape lets each backend do exactly what its native widget
//! system makes easy: hand back a `raw_window_handle` and forget
//! about GPU concerns.
//!
//! # Per-backend strategy
//!
//! - **Web**: a `<canvas>` element, exposed as `WebCanvasWindowHandle`
//!   + `WebDisplayHandle`. Author creates whatever context they want
//!   (`wgpu::Instance::create_surface(&handle)`,
//!   `canvas.getContext("webgl2")`, `canvas.getContext("2d")`).
//! - **Android**: a `SurfaceView`, exposed as `AndroidNdkWindowHandle`
//!   (`ANativeWindow_fromSurface`) + `AndroidDisplayHandle`. Surface
//!   creation/destruction follows `SurfaceHolder.Callback`'s
//!   `surfaceCreated` / `surfaceChanged` / `surfaceDestroyed` events,
//!   which fire `on_ready` / `on_resize` / `on_lost` respectively.
//! - **iOS**: not yet implemented — would expose the view's
//!   `CAMetalLayer` as `AppKitWindowHandle`/`UiKitWindowHandle`.
//! - **Linux (GTK4)**: a `GtkGLArea`, exposed as a [`GlTarget`] rather
//!   than a window handle — see below.
//! - **Windows**: a DirectComposition visual in a composition tree
//!   over the host window, exposed through the optional
//!   [`ComposedTarget`] capability ([`GraphicsSurface::composed_target`])
//!   rather than a window handle — `window_handle()` deliberately
//!   answers `Unavailable`. See that trait's docs for the host-side
//!   contract (commit-after-configure, visibility).
//!
//! # Why the target is an enum
//!
//! The strategy above assumes each graphics widget owns a native
//! window/layer to wrap in a `raw_window_handle`. GTK4 breaks that
//! assumption: it removed GTK3's per-widget native windows, so the
//! entire toplevel is one native surface that GTK renders every widget
//! into with its own GSK renderer. There is no per-widget
//! `wl_surface`/XID, and the toplevel's handle is both the wrong rect
//! and already owned by GTK's renderer — a second wgpu swapchain on it
//! fights GTK for the buffer. GTK's sanctioned escape hatch
//! (`GtkGLArea`) yields a GL *context* + framebuffer, which
//! [`GraphicsSurface`] structurally cannot represent.
//!
//! So [`OnReadyEvent::target`] is a [`GraphicsTarget`] — `RawWindow`
//! on every backend that has a real handle, `Gl` where the toolkit only
//! lends a context. Making it an enum rather than "surface, sometimes
//! fabricated" keeps the impossibility *visible* at the type level:
//! an author that only supports swapchains gets `None` from
//! [`OnReadyEvent::into_surface`] and degrades, instead of a handle
//! that errors inside `create_surface` or hijacks the whole window.
//!
//! # Lifecycle
//!
//! The framework guarantees one of the following sequences:
//!
//! - Mount → `on_ready` → (`on_resize` …)* → unmount
//! - Mount → `on_ready` → `on_lost` → `on_ready` → … → unmount
//!   (Android's SurfaceView destroys + recreates its surface on
//!   backgrounding; on `on_lost` the author MUST drop any handle
//!   state derived from the previous surface, then expect a new
//!   `on_ready` when it returns.)
//!
//! `on_resize` always fires *after* the size has actually changed —
//! it's not invoked with the initial size (use `on_ready.size` for
//! that).

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

/// The drawable surface handed to the author in `on_ready`. Wraps
/// a backend-provided handle that implements both `HasWindowHandle`
/// and `HasDisplayHandle` from [`raw_window_handle`], so it plugs
/// into any GPU library that takes those traits.
///
/// `Clone` — cheap (a refcount bump). Author code typically
/// captures it once into render state.
///
/// # Send + Sync
///
/// Wgpu's native surface APIs require `Send + Sync` window
/// handles, so this type carries those bounds unconditionally.
/// Native backends (iOS / Android / desktop) satisfy them
/// naturally. The web backend's `CanvasSurfaceProvider` wraps an
/// `HtmlCanvasElement`, which is structurally `!Send + !Sync`, but
/// adds `unsafe impl Send + Sync` to the wrapper — sound because
/// wasm32 is single-threaded, so the bounds are vacuously safe
/// (no second thread can observe a torn read). One unified type
/// across targets; no cfg gates on the framework side.
#[derive(Clone)]
pub struct GraphicsSurface {
    pub(crate) inner: Arc<dyn SurfaceProvider + Send + Sync>,
    /// Optional composition-tree capability — see [`ComposedTarget`].
    /// `None` on backends whose surface is a real native window/view.
    pub(crate) composed: Option<Arc<dyn ComposedTarget>>,
}

impl GraphicsSurface {
    pub fn new(inner: Arc<dyn SurfaceProvider + Send + Sync>) -> Self {
        Self { inner, composed: None }
    }

    /// Construct a surface whose drawable is a compositor visual rather
    /// than a native window. `inner` still provides the display handle
    /// (and typically answers `window_handle()` with
    /// `HandleError::Unavailable` — there is deliberately no HWND/view
    /// to hand out, or a consumer that ignored [`Self::composed_target`]
    /// would build a swapchain over the whole host window).
    pub fn new_composed(
        inner: Arc<dyn SurfaceProvider + Send + Sync>,
        composed: Arc<dyn ComposedTarget>,
    ) -> Self {
        Self { inner, composed: Some(composed) }
    }

    /// The composition-visual capability, when this surface is backed
    /// by one. Hosts check this FIRST and fall back to the
    /// raw-window-handle path when it's `None`.
    pub fn composed_target(&self) -> Option<&Arc<dyn ComposedTarget>> {
        self.composed.as_ref()
    }
}

/// Optional capability of a [`GraphicsSurface`]: the drawable is a
/// visual in the platform's retained composition tree (Windows
/// DirectComposition today) instead of a native window/view.
///
/// Why this exists: positioning a *window*-backed surface means moving
/// an OS window, which composes on its own schedule — during fast
/// scrolling the surface visibly detaches from painted content around
/// it. A composition visual moves by a property write + commit that is
/// atomic with the compositor's next frame, so the backend can keep the
/// surface glued to the scene by construction. wgpu supports mounting a
/// swapchain directly on such a visual
/// (`SurfaceTargetUnsafe::CompositionVisual`), which is why the raw
/// pointer is exposed rather than a `raw-window-handle` type (that
/// crate has no composition-visual variant).
///
/// `Send + Sync` for the same reason [`GraphicsSurface`] carries those
/// bounds (wgpu requires them); backends that are single-threaded by
/// construction document their `unsafe impl`s accordingly.
pub trait ComposedTarget: Send + Sync + 'static {
    /// Raw pointer to the platform visual (`IDCompositionVisual` on
    /// Windows). The provider owns a reference for the surface's
    /// lifetime; consumers that outlive it must add their own
    /// (wgpu's `CompositionVisual` target does).
    fn visual(&self) -> std::ptr::NonNull<std::ffi::c_void>;

    /// Commit pending composition-tree changes. A swapchain bound to
    /// the visual (`SetContent`) only appears at the next device
    /// commit, so hosts MUST call this after (re)configuring their
    /// surface — the backend's own commits happen on layout/scroll and
    /// may never fire for a static scene.
    fn commit(&self);

    /// Whether the visual currently sits in a visible part of the
    /// tree (portal-hidden subtrees report `false`). Replaces the
    /// `IsWindowVisible`-style walk hosts use to skip rendering for
    /// window-backed surfaces.
    fn is_visible(&self) -> bool;
}

impl HasWindowHandle for GraphicsSurface {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        self.inner.window_handle()
    }
}

impl HasDisplayHandle for GraphicsSurface {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        self.inner.display_handle()
    }
}

/// Trait the per-backend surface object implements. Auto-blanketed
/// for any type that already implements the two raw-window-handle
/// traits — backends just hand the framework an `Rc<MyHandleType>`
/// (web) or `Arc<MyHandleType>` (native) and the auto-impl makes
/// it a `SurfaceProvider`.
///
/// `'static` because the surface outlives the call chain and is
/// stashed in the user's render state.
pub trait SurfaceProvider: HasWindowHandle + HasDisplayHandle + 'static {}
impl<T: HasWindowHandle + HasDisplayHandle + 'static> SurfaceProvider for T {}

/// A live OpenGL context the backend owns and lends to the author,
/// for toolkits that composite every widget into one native surface
/// and therefore cannot hand out a per-widget window handle.
///
/// GTK4 is the motivating case: it removed GTK3's per-widget native
/// windows, so the only raw handle obtainable is the *toplevel's* —
/// already owned and continuously presented by GTK's own renderer, so
/// a second swapchain configured on it fights GTK for the buffer.
/// GTK's sanctioned escape hatch is `GtkGLArea`, which hands out a
/// `GdkGLContext` + a framebuffer. That's a GL *context*, not a window
/// handle, so [`GraphicsSurface`] cannot express it.
///
/// # Thread affinity — why `Rc`, not `Arc`
///
/// A GL context is current on *one* thread at a time and is invalid to
/// touch from any other, so unlike [`GraphicsSurface`] (which wgpu
/// requires be `Send + Sync`) this type is deliberately neither. The
/// refcount matches the constraint the API already has rather than
/// papering over it with an `unsafe impl Send`.
///
/// # Contract for the author
///
/// Every call into a GPU library built on this context — creation,
/// rendering, *and drop* — must happen between [`make_current`] and
/// the return to the backend's event loop. wgpu's GLES HAL states this
/// explicitly for externally-adopted contexts
/// (`gles::Adapter::new_external`): it holds no EGL handle of its own
/// and never makes the context current for you.
///
/// [`make_current`]: GlTarget::make_current
#[derive(Clone)]
pub struct GlTarget {
    inner: Rc<dyn GlContextProvider>,
}

impl GlTarget {
    pub fn new(inner: Rc<dyn GlContextProvider>) -> Self {
        Self { inner }
    }

    /// Resolve a GL entry point by name, for a loader-function-based
    /// GPU library (`glow::Context::from_loader_function`,
    /// `wgpu_hal::gles::Adapter::new_external`, `epoxy`, `gl::load_with`).
    /// Returns null for an unavailable symbol, as GL loaders expect.
    pub fn get_proc_address(&self, symbol: &str) -> *const std::ffi::c_void {
        self.inner.get_proc_address(symbol)
    }

    /// Make the context current on the calling thread AND bind the
    /// framebuffer named by [`framebuffer`](Self::framebuffer). Must be
    /// called before any GL work; cheap enough to call per frame, and
    /// the backend may have swapped the framebuffer out from under a
    /// resize since the last frame.
    pub fn make_current(&self) {
        self.inner.make_current();
    }

    /// The GL framebuffer object the author must render into — NOT the
    /// default framebuffer (`0`). The backend composites this into the
    /// rest of its widget tree.
    ///
    /// Re-read after every [`make_current`](Self::make_current): the id
    /// changes when the backend reallocates its buffers on resize, and
    /// rendering into the stale one draws into a freed attachment.
    pub fn framebuffer(&self) -> u32 {
        self.inner.framebuffer()
    }

    /// Row order of the lent framebuffer — which edge of the image row
    /// 0 sits on. See [`FramebufferOrigin`]; on GL this is
    /// [`BottomLeft`](FramebufferOrigin::BottomLeft) and a renderer
    /// working in top-left coordinates MUST flip vertically.
    pub fn origin(&self) -> FramebufferOrigin {
        self.inner.origin()
    }

    /// Register the per-frame draw. The backend invokes `cb` **with the
    /// GL context current and the target framebuffer bound**, each time
    /// it composites this drawable into its widget tree.
    ///
    /// This is where drawing MUST happen. A toolkit like GTK4 only
    /// composites what is rendered *during its own render pass*
    /// (`GtkGLArea::render`); a frame drawn out-of-band (via
    /// [`make_current`](Self::make_current)) between passes is **not
    /// reliably composited** — the widget keeps showing the last frame
    /// the toolkit actually captured. So the model is: mutate your
    /// GPU-side state whenever you like, call [`present`](Self::present)
    /// to schedule a pass, and do the blit-to-framebuffer *here*.
    ///
    /// `cb` runs on the UI thread. Re-registering replaces the previous
    /// callback. The context is already current inside `cb`, so it need
    /// not call `make_current` itself.
    pub fn on_render(&self, cb: Box<dyn FnMut()>) {
        self.inner.set_render_callback(cb);
    }

    /// Request that the backend composite a frame. On GTK this schedules
    /// the render pass that will invoke your [`on_render`](Self::on_render)
    /// callback — the actual drawing happens there, not before this call.
    ///
    /// The raw-window path has no equivalent because there the author
    /// owns a swapchain and calls `present()` on the frame itself.
    pub fn present(&self) {
        self.inner.present();
    }
}

/// Which edge of the image row 0 of a framebuffer sits on.
///
/// Renderers overwhelmingly work in `TopLeft` coordinates (wgpu, Metal,
/// D3D, every 2D scene graph in this repo). GL is the odd one out: its
/// framebuffer origin is the bottom-left, so a top-left-origin image
/// written into a GL framebuffer is displayed upside down unless the
/// renderer flips.
///
/// This is reported as *data on the target* rather than left for
/// consumers to infer from the platform, so that folding the flip in
/// costs a sign in an existing projection matrix and never a
/// `if cfg!(linux)` at a call site. It's the same shape as
/// [`OnReadyEvent::scale`]: something about this particular drawable
/// that the renderer's base transform has to account for.
///
/// Verified rather than assumed — `texture_from_raw` does NOT flip
/// indices, and GDK's compositing of the GL texture does. See
/// `backend-linux/tests/gl_target_adoption.rs`, which measures both the
/// raw framebuffer layout and what GTK actually puts on screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FramebufferOrigin {
    TopLeft,
    BottomLeft,
}

/// Trait the per-backend GL context object implements. See
/// [`GlTarget`] for the calling contract — the wrapper's methods are
/// one-to-one with these.
pub trait GlContextProvider: 'static {
    fn get_proc_address(&self, symbol: &str) -> *const std::ffi::c_void;
    fn make_current(&self);
    fn framebuffer(&self) -> u32;
    fn origin(&self) -> FramebufferOrigin;
    /// Store the per-frame draw callback. Interior-mutable: called
    /// through a shared `&self`. See [`GlTarget::on_render`].
    fn set_render_callback(&self, cb: Box<dyn FnMut()>);
    fn present(&self);
}

/// What the backend hands the author to render into.
///
/// Most backends own a native window/layer per graphics widget and
/// yield [`RawWindow`](Self::RawWindow) — the author calls
/// `wgpu::Instance::create_surface(&surface)` and drives a swapchain.
/// Toolkits that composite every widget into a single native surface
/// (GTK4) cannot do that and yield [`Gl`](Self::Gl) instead.
///
/// Authors that only support the swapchain model should
/// [`into_surface`](OnReadyEvent::into_surface) and degrade gracefully
/// on `None` rather than assume a surface is always there — that's the
/// whole reason this is an enum and not a struct field.
pub enum GraphicsTarget {
    RawWindow(GraphicsSurface),
    Gl(GlTarget),
}

/// Event delivered to `on_ready`. The surface is in the layout tree
/// and has a real size; the author can build their GPU state
/// synchronously.
pub struct OnReadyEvent {
    pub target: GraphicsTarget,
    /// Drawable size in physical pixels. On web this already
    /// accounts for `devicePixelRatio`; native backends report the
    /// pixel-buffer size directly. Authors should size their
    /// swapchain / depth buffer to match.
    pub size: (u32, u32),
    /// Device pixel ratio (physical px per logical point) of the
    /// surface — `backingScaleFactor` (macOS), `UIScreen.scale` (iOS),
    /// display density (Android), `devicePixelRatio` (web). Renderers
    /// drawing a LOGICAL-coordinate scene into the physical-pixel
    /// `size` must apply `scale` as a base transform, or the content
    /// under-fills on a HiDPI surface (e.g. retina → top-left quarter).
    /// `1.0` from a backend means "not yet reported" (renders at
    /// physical scale, the historical behavior).
    pub scale: f32,
}

impl OnReadyEvent {
    /// Borrow the raw-window handle, or `None` on a GL-context backend.
    pub fn surface(&self) -> Option<&GraphicsSurface> {
        match &self.target {
            GraphicsTarget::RawWindow(s) => Some(s),
            GraphicsTarget::Gl(_) => None,
        }
    }

    /// Take the raw-window handle, or `None` on a GL-context backend.
    /// `wgpu::Instance::create_surface` wants it by value (it yields a
    /// `Surface<'static>`), so this is the usual accessor.
    pub fn into_surface(self) -> Option<GraphicsSurface> {
        match self.target {
            GraphicsTarget::RawWindow(s) => Some(s),
            GraphicsTarget::Gl(_) => None,
        }
    }

    /// Borrow the lent GL context, or `None` on a raw-window backend.
    pub fn gl(&self) -> Option<&GlTarget> {
        match &self.target {
            GraphicsTarget::Gl(g) => Some(g),
            GraphicsTarget::RawWindow(_) => None,
        }
    }
}

/// Event delivered to `on_resize`. Fires whenever the drawable
/// changes size (window resize on web, orientation change /
/// split-screen on Android, layout change anywhere). NOT fired for
/// the initial size — read `on_ready.size` for that.
pub struct OnResizeEvent {
    pub size: (u32, u32),
    /// Device pixel ratio — see [`OnReadyEvent::scale`]. Can change
    /// when a window moves between displays of different density.
    pub scale: f32,
}

/// Closure invoked once the platform surface is ready (or every time
/// it becomes ready again after an `on_lost`). Authors typically
/// build their wgpu surface + adapter + device + pipelines here.
///
/// `FnMut` — Android can fire `on_lost` then `on_ready` again, so
/// the closure must support multiple invocations. State persists
/// across calls (closures move-capture).
pub type OnReady = Box<dyn FnMut(OnReadyEvent)>;

/// Closure invoked when the drawable size changes after `on_ready`.
/// Authors reconfigure their swapchain here. NOT called for the
/// initial size.
pub type OnResize = Box<dyn FnMut(OnResizeEvent)>;

/// Closure invoked when the surface becomes invalid (Android
/// `surfaceDestroyed`, web context-lost, iOS Metal layer reclaimed).
/// Authors MUST drop every handle derived from the previous surface
/// — wgpu Surface, swapchain, anything that holds a window-handle
/// borrow — before returning. A new `on_ready` follows when the
/// surface comes back; if the primitive is being unmounted, no
/// `on_ready` follows.
pub type OnLost = Box<dyn FnMut()>;

/// Handle exposed to the parent via `Ref<GraphicsHandle>`. No
/// methods today — the surface-provider model means everything
/// flows through the lifecycle callbacks. Reserved for future
/// imperative ops (e.g. force-rebuild surface).
#[derive(Clone)]
pub struct GraphicsHandle {
    #[allow(dead_code)]
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn GraphicsOps,
}

impl GraphicsHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn GraphicsOps) -> Self {
        Self { node, ops }
    }

    /// Borrow the backend-specific node Rc the handle wraps.
    /// Authors using a non-portable backend extension (e.g. the
    /// wgpu preview's `register_graphics_drawer`) downcast this
    /// to the concrete `Backend::Node` to install per-node state.
    /// Portable code doesn't need to call this — the lifecycle
    /// callbacks (`on_ready` / `on_resize` / `on_lost`) handle
    /// it on every supported backend.
    pub fn node(&self) -> &Rc<dyn Any> {
        &self.node
    }
}

pub trait GraphicsOps {
    // Reserved for future imperative ops.
}


#[cfg(test)]
mod tests {
    use super::*;

    struct FakeGl {
        presented: std::cell::Cell<u32>,
        render_cb: std::cell::RefCell<Option<Box<dyn FnMut()>>>,
    }

    impl GlContextProvider for FakeGl {
        fn get_proc_address(&self, symbol: &str) -> *const std::ffi::c_void {
            // Mimics a real loader: known symbol resolves, unknown is null.
            if symbol == "glClear" {
                1 as *const std::ffi::c_void
            } else {
                std::ptr::null()
            }
        }
        fn make_current(&self) {}
        fn framebuffer(&self) -> u32 {
            7
        }
        fn origin(&self) -> FramebufferOrigin {
            FramebufferOrigin::BottomLeft
        }
        fn set_render_callback(&self, cb: Box<dyn FnMut()>) {
            *self.render_cb.borrow_mut() = Some(cb);
        }
        fn present(&self) {
            self.presented.set(self.presented.get() + 1);
            // Mimic a backend that composites by invoking the registered
            // draw synchronously (a real backend does it in its render pass).
            if let Some(cb) = self.render_cb.borrow_mut().as_mut() {
                cb();
            }
        }
    }

    struct FakeWindow;

    impl HasWindowHandle for FakeWindow {
        fn window_handle(
            &self,
        ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
            Err(raw_window_handle::HandleError::NotSupported)
        }
    }

    impl HasDisplayHandle for FakeWindow {
        fn display_handle(
            &self,
        ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
            Err(raw_window_handle::HandleError::NotSupported)
        }
    }

    fn gl_event() -> OnReadyEvent {
        OnReadyEvent {
            target: GraphicsTarget::Gl(GlTarget::new(Rc::new(FakeGl {
                presented: std::cell::Cell::new(0),
                render_cb: std::cell::RefCell::new(None),
            }))),
            size: (800, 600),
            scale: 1.0,
        }
    }

    fn window_event() -> OnReadyEvent {
        OnReadyEvent {
            target: GraphicsTarget::RawWindow(GraphicsSurface::new(Arc::new(FakeWindow))),
            size: (800, 600),
            scale: 2.0,
        }
    }

    /// The whole reason `target` is an enum: a renderer that only knows
    /// how to drive a swapchain must be able to detect that there ISN'T
    /// one and step aside. If these ever returned a fabricated surface,
    /// such a renderer would fail deep inside `create_surface` (or worse,
    /// hijack the toolkit's own window) instead of degrading.
    #[test]
    fn gl_target_yields_no_raw_window_surface() {
        assert!(gl_event().surface().is_none());
        assert!(gl_event().into_surface().is_none());
        assert!(gl_event().gl().is_some());
    }

    #[test]
    fn raw_window_target_yields_a_surface_and_no_gl_context() {
        assert!(window_event().surface().is_some());
        assert!(window_event().into_surface().is_some());
        assert!(window_event().gl().is_none());
    }

    struct FakeComposed {
        committed: std::sync::atomic::AtomicU32,
        visible: std::sync::atomic::AtomicBool,
    }

    impl ComposedTarget for FakeComposed {
        fn visual(&self) -> std::ptr::NonNull<std::ffi::c_void> {
            std::ptr::NonNull::new(0x1234 as *mut std::ffi::c_void).unwrap()
        }
        fn commit(&self) {
            self.committed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        fn is_visible(&self) -> bool {
            self.visible.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// Hosts branch on `composed_target()` FIRST — a plain surface must
    /// answer `None` (fall through to raw-window-handle), a composed
    /// one must expose the live capability object, and clones must
    /// address the SAME capability (the host stashes a clone in its
    /// render state and reads `is_visible` every frame).
    #[test]
    fn composed_target_capability_is_none_by_default_and_shared_when_present() {
        assert!(GraphicsSurface::new(Arc::new(FakeWindow)).composed_target().is_none());

        let cap = Arc::new(FakeComposed {
            committed: std::sync::atomic::AtomicU32::new(0),
            visible: std::sync::atomic::AtomicBool::new(true),
        });
        let surface = GraphicsSurface::new_composed(Arc::new(FakeWindow), cap.clone());
        let clone = surface.clone();

        let ct = clone.composed_target().expect("composed surface exposes the capability");
        assert_eq!(ct.visual().as_ptr() as usize, 0x1234);
        ct.commit();
        assert_eq!(cap.committed.load(std::sync::atomic::Ordering::SeqCst), 1);
        cap.visible.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(!ct.is_visible(), "clone must observe the backend's live visibility");
    }

    /// Every accessor on the wrapper must reach the backend's provider.
    /// A defaulted or swallowed method here is invisible at the call
    /// site and shows up as a blank or upside-down GPU surface.
    #[test]
    fn gl_target_forwards_every_accessor_to_the_provider() {
        let provider = Rc::new(FakeGl { presented: std::cell::Cell::new(0), render_cb: std::cell::RefCell::new(None) });
        let target = GlTarget::new(provider.clone());

        assert!(!target.get_proc_address("glClear").is_null());
        assert!(target.get_proc_address("glNope").is_null());
        assert_eq!(target.framebuffer(), 7);
        assert_eq!(target.origin(), FramebufferOrigin::BottomLeft);

        target.present();
        target.present();
        assert_eq!(provider.presented.get(), 2, "present() must reach the backend");
    }

    /// `GlTarget` is `Clone` so a renderer can stash it in its own
    /// state; clones must address the SAME context, not a detached copy.
    #[test]
    fn cloning_a_gl_target_shares_one_context() {
        let provider = Rc::new(FakeGl { presented: std::cell::Cell::new(0), render_cb: std::cell::RefCell::new(None) });
        let target = GlTarget::new(provider.clone());
        let clone = target.clone();
        clone.present();
        target.present();
        assert_eq!(provider.presented.get(), 2);
    }
}
