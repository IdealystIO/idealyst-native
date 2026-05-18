//! Graphics primitive — a backend-provided platform surface the
//! author renders to with their own GPU library.
//!
//! The framework's job is narrow on purpose: stand up a drawable
//! surface in the layout (`<canvas>` on web, `SurfaceView` on
//! Android, `UIView` + `CAMetalLayer` on iOS), expose it as a
//! standard [`raw_window_handle`] handle, and notify the author when
//! it's ready / resized / lost. Everything past that — picking a GPU
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
//! through `Rc<dyn Any>` to keep framework-core wgpu-free. The new
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

use crate::{Bound, Primitive, Ref, RefFill};
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
}

impl GraphicsSurface {
    pub fn new(inner: Arc<dyn SurfaceProvider + Send + Sync>) -> Self {
        Self { inner }
    }
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

/// Event delivered to `on_ready`. The surface is in the layout tree
/// and has a real size; the author can call
/// `wgpu::Instance::create_surface(&event.surface)` synchronously.
pub struct OnReadyEvent {
    pub surface: GraphicsSurface,
    /// Drawable size in physical pixels. On web this already
    /// accounts for `devicePixelRatio`; native backends report the
    /// pixel-buffer size directly. Authors should size their
    /// swapchain / depth buffer to match.
    pub size: (u32, u32),
}

/// Event delivered to `on_resize`. Fires whenever the drawable
/// changes size (window resize on web, orientation change /
/// split-screen on Android, layout change anywhere). NOT fired for
/// the initial size — read `on_ready.size` for that.
pub struct OnResizeEvent {
    pub size: (u32, u32),
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
}

pub trait GraphicsOps {
    // Reserved for future imperative ops.
}

/// Construct a Graphics surface primitive. `on_ready` is required;
/// `on_resize` and `on_lost` are optional and default to no-ops.
/// Use the builder methods below to attach them.
pub fn graphics<F>(on_ready: F) -> Bound<GraphicsHandle>
where
    F: FnMut(OnReadyEvent) + 'static,
{
    Bound::new(Primitive::Graphics {
        on_ready: Box::new(on_ready),
        on_resize: Box::new(|_| {}),
        on_lost: Box::new(|| {}),
        style: None,
        ref_fill: None,
    })
}

impl Bound<GraphicsHandle> {
    pub fn on_resize<F: FnMut(OnResizeEvent) + 'static>(mut self, f: F) -> Self {
        if let Primitive::Graphics { on_resize, .. } = &mut self.primitive {
            *on_resize = Box::new(f);
        }
        self
    }

    pub fn on_lost<F: FnMut() + 'static>(mut self, f: F) -> Self {
        if let Primitive::Graphics { on_lost, .. } = &mut self.primitive {
            *on_lost = Box::new(f);
        }
        self
    }

    pub fn bind(mut self, r: Ref<GraphicsHandle>) -> Self {
        if let Primitive::Graphics { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Graphics(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
