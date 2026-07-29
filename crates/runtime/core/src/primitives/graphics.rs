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

use crate::{Bound, Element, Ref, RefFill};

// The data/handle/Ops types of this primitive moved to `runtime-shared`
// (the walker-free half); this file keeps the Element/Bound builder
// surface (and its tests). The wildcard re-export preserves every old
// path.
pub use runtime_shared::primitives::graphics::*;

/// Construct a Graphics surface primitive. `on_ready` is required;
/// `on_resize` and `on_lost` are optional and default to no-ops.
/// Use the builder methods below to attach them.
#[cfg(feature = "prim-graphics")]
pub fn graphics<F>(on_ready: F) -> Bound<GraphicsHandle>
where
    F: FnMut(OnReadyEvent) + 'static,
{
    Bound::new(Element::Graphics {
        on_ready: Box::new(on_ready),
        on_resize: Box::new(|_| {}),
        on_lost: Box::new(|| {}),
        style: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
        #[cfg(feature = "robot")]
        test_id: None,
    })
}

impl Bound<GraphicsHandle> {
    pub fn on_resize<F: FnMut(OnResizeEvent) + 'static>(mut self, mut f: F) -> Self {
        if let Element::Graphics { on_resize, .. } = &mut self.primitive {
            // Born batched — see `reactive::cycle`.
            *on_resize = Box::new(move |e: OnResizeEvent| crate::cycle(|| f(e)));
        }
        self
    }

    pub fn on_lost<F: FnMut() + 'static>(mut self, mut f: F) -> Self {
        if let Element::Graphics { on_lost, .. } = &mut self.primitive {
            // Born batched — see `reactive::cycle`.
            *on_lost = Box::new(move || crate::cycle(|| f()));
        }
        self
    }

    pub fn bind(mut self, r: Ref<GraphicsHandle>) -> Self {
        if let Element::Graphics { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Graphics(Box::new(move |h| r.fill(h))));
        }
        self
    }
}
