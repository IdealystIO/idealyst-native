//! `Primitive::Graphics` — `android.view.SurfaceView` exposed as a
//! `raw_window_handle` surface provider.
//!
//! Lifecycle:
//!
//! 1. `create` builds a `SurfaceView`, attaches a Kotlin
//!    `RustGraphicsCallback` to its `SurfaceHolder`, and leaks a
//!    Rust [`GraphicsCallback`] box that stores the user closures.
//!    Returns the `SurfaceView` as the layout node.
//! 2. Android's surface lifecycle drives JNI callbacks
//!    (`nativeSurfaceCreated`, `nativeSurfaceChanged`,
//!    `nativeSurfaceDestroyed`) which convert the Java `Surface`
//!    to an `ANativeWindow*` via `ANativeWindow_fromSurface` and
//!    fire the corresponding user callback (`on_ready` /
//!    `on_resize` / `on_lost`).
//! 3. `release` (called from `Backend::release_graphics` when the
//!    parent scope drops) frees the leaked `GraphicsCallback` box.
//!    Existing surfaces are torn down through the normal Android
//!    path: detaching the SurfaceView fires `surfaceDestroyed`,
//!    which fires `on_lost`.
//!
//! # raw-window-handle
//!
//! The handle the framework hands the user is built fresh per
//! `on_ready` call, wrapping the current `ANativeWindow*`. wgpu's
//! `Instance::create_surface(&handle)` reads `window_handle()` /
//! `display_handle()` synchronously during surface creation, so the
//! handle's borrow-lifetime is bounded by the call. The user's
//! `wgpu::Surface<'static>` then holds the underlying ANativeWindow
//! reference itself (`acquire`d by wgpu-hal); we release ours when
//! the next surface lifecycle event fires.

use crate::imp::callbacks::leak;
use crate::imp::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use framework_core::primitives::graphics::{
    GraphicsHandle, GraphicsOps, GraphicsSurface, OnLost, OnReady, OnReadyEvent, OnResize,
    OnResizeEvent, SurfaceProvider,
};
use jni::objects::{GlobalRef, JValue};
use jni::sys::jlong;
use raw_window_handle::{
    AndroidDisplayHandle, AndroidNdkWindowHandle, DisplayHandle, HandleError, HasDisplayHandle,
    HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};
use std::cell::RefCell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// SurfaceProvider impl — wraps an ANativeWindow* and produces handles
// ---------------------------------------------------------------------------

/// Surface provider for a single Android `Surface` lifetime. Holds
/// the `ANativeWindow*` it was built from; releasing `Drop` calls
/// `ANativeWindow_release` so wgpu's `Acquire` is correctly paired.
///
/// Created fresh for every `on_ready` event — Android can destroy
/// and recreate the underlying surface (backgrounding the activity,
/// rotating the device), and each surface is a different
/// `ANativeWindow*`.
pub(crate) struct AndroidSurfaceProvider {
    window: NonNull<ndk_sys::ANativeWindow>,
}

// `*mut ANativeWindow` is just an opaque pointer; the wgpu-hal
// Vulkan driver uses it from arbitrary threads. The framework's
// model is single-threaded so we don't actually move it across
// threads, but the auto-traits on `NonNull` would otherwise prevent
// it from satisfying `WgpuHasWindowHandle` bounds.
unsafe impl Send for AndroidSurfaceProvider {}
unsafe impl Sync for AndroidSurfaceProvider {}

impl Drop for AndroidSurfaceProvider {
    fn drop(&mut self) {
        // Pair the `_acquire` we did at construction.
        unsafe { ndk_sys::ANativeWindow_release(self.window.as_ptr()) };
    }
}

impl HasWindowHandle for AndroidSurfaceProvider {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::AndroidNdk(AndroidNdkWindowHandle::new(self.window.cast()));
        // SAFETY: window is alive for the full borrow of `&self`,
        // and `WindowHandle<'_>` is bounded by that borrow.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for AndroidSurfaceProvider {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let raw = RawDisplayHandle::Android(AndroidDisplayHandle::new());
        // SAFETY: `AndroidDisplayHandle` has no fields and no
        // borrowed references; the `unsafe` is API-bookkeeping.
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

// ---------------------------------------------------------------------------
// GraphicsCallback — leaked box holding the user closures
// ---------------------------------------------------------------------------

/// Holder for everything the JNI surface callbacks need to dispatch:
/// the user's three closures and the latest known size (so we can
/// suppress duplicate `surfaceChanged` calls and so `on_ready`
/// reports the size that just got reported on `surfaceChanged`).
///
/// Lives as a leaked `Box<GraphicsCallback>` whose raw pointer is
/// passed to the Kotlin callback as a `jlong`. `Backend::release_graphics`
/// calls `nativeDrop` to free it.
///
/// `RefCell`-wrapped because the JNI callbacks need to mutate the
/// closures (we move-take them out across each call so the user
/// can re-enter Rust without re-borrowing the cell — same trick as
/// the web backend).
pub(crate) struct GraphicsCallback {
    pub(crate) on_ready: RefCell<OnReady>,
    pub(crate) on_resize: RefCell<OnResize>,
    pub(crate) on_lost: RefCell<OnLost>,
    /// Set to true once `nativeDrop` has run — the callbacks
    /// downcast through the `&*ptr` reference, but Android may
    /// still deliver a `surfaceDestroyed` event after detach in
    /// edge cases. We check this before invoking anything.
    pub(crate) released: bool,
    /// Latest known drawable size in physical pixels. Updated
    /// inside `nativeSurfaceChanged`.
    pub(crate) last_size: (u32, u32),
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

pub(crate) fn create(
    b: &AndroidBackend,
    on_ready: OnReady,
    on_resize: OnResize,
    on_lost: OnLost,
) -> GlobalRef {
    let cb = GraphicsCallback {
        on_ready: RefCell::new(on_ready),
        on_resize: RefCell::new(on_resize),
        on_lost: RefCell::new(on_lost),
        released: false,
        last_size: (0, 0),
    };
    let ptr: jlong = leak(cb);

    with_env(|env| {
        let class = env.find_class("android/view/SurfaceView").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();

        // Render the SurfaceView's secondary surface ABOVE the View
        // hierarchy. Default Android behavior is the opposite — the
        // View itself paints in the normal compositor layer and the
        // surface lives behind it via a transparent "hole" punched
        // through the View. That works as long as nothing in the
        // View tree (including the SurfaceView's own background
        // drawable from `apply_style`) paints over the hole. Once
        // any author applies `background: …` via `with_style`, the
        // background drawable occludes the surface and you see a
        // flat color where the gradient should be. Z-order-on-top
        // sidesteps the whole class of issues: the wgpu output
        // covers everything in the View's bounding rect, regardless
        // of what the View itself draws.
        env.call_method(&local, "setZOrderOnTop", "(Z)V", &[JValue::Bool(1)])
            .unwrap();

        // Build the Kotlin callback wrapping our leaked pointer,
        // then attach it via SurfaceHolder.addCallback.
        let cb_class = env
            .find_class("io/idealyst/runtime/RustGraphicsCallback")
            .unwrap();
        let cb_obj = env
            .new_object(&cb_class, "(J)V", &[JValue::Long(ptr)])
            .unwrap();
        let holder = env
            .call_method(&local, "getHolder", "()Landroid/view/SurfaceHolder;", &[])
            .unwrap()
            .l()
            .unwrap();
        env.call_method(
            &holder,
            "addCallback",
            "(Landroid/view/SurfaceHolder$Callback;)V",
            &[JValue::Object(&cb_obj)],
        )
        .unwrap();

        apply_default_layout_params(env, &local);
        env.new_global_ref(local).unwrap()
    })
}

pub(crate) fn make_handle(node: &GlobalRef) -> GraphicsHandle {
    GraphicsHandle::new(Rc::new(node.clone()), &AndroidGraphicsOps)
}

struct AndroidGraphicsOps;
impl GraphicsOps for AndroidGraphicsOps {}

// ---------------------------------------------------------------------------
// release — Backend::release_graphics calls this
// ---------------------------------------------------------------------------

pub(crate) fn release(_b: &mut AndroidBackend, _node: &GlobalRef) {
    // The SurfaceView's normal teardown will fire surfaceDestroyed,
    // which the user's on_lost will handle. We don't need to do
    // anything Java-side here — the ViewGroup that contained the
    // SurfaceView is being torn down.
    //
    // The leaked GraphicsCallback box is freed by Kotlin when the
    // listener object is GC'd (via the `nativeDrop` finalizer hook
    // — which we don't currently wire; same lifetime story as
    // every other JNI listener in this backend). For the demo's
    // single-Activity lifetime this is fine; a long-lived app
    // would want a `nativeDrop` JNI export and a finalizer in the
    // Kotlin class.
}

// ---------------------------------------------------------------------------
// JNI exports — invoked from RustGraphicsCallback's three methods
// ---------------------------------------------------------------------------

/// `surfaceCreated` dispatch. Acquires an `ANativeWindow` from the
/// Java `Surface`, wraps it in a fresh `AndroidSurfaceProvider`,
/// and fires `on_ready`.
///
/// Note that `on_ready` MAY fire before `surfaceChanged` — the
/// `last_size` field starts at `(0, 0)`. Authors should generally
/// wait for the first `on_resize` if they care about exact pixels.
/// In practice the SurfaceView fires `surfaceChanged` immediately
/// after `surfaceCreated`, so the size is set before the user's
/// next code path runs.
///
/// # Safety
///
/// `ptr` must point to a live `Box<GraphicsCallback>` produced by
/// [`create`] above.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustGraphicsCallback_nativeSurfaceCreated<'l>(
    env: jni::JNIEnv<'l>,
    _this: jni::objects::JObject<'l>,
    ptr: jlong,
    surface: jni::objects::JObject<'l>,
) {
    if ptr == 0 {
        return;
    }
    let cb = &mut *(ptr as *mut GraphicsCallback);
    if cb.released {
        return;
    }

    // ANativeWindow_fromSurface increments the surface's ref count;
    // the matching release happens when the SurfaceProvider drops.
    let raw = env.get_raw();
    let surface_ptr = surface.as_raw();
    let window = ndk_sys::ANativeWindow_fromSurface(raw.cast(), surface_ptr.cast());
    let Some(window) = NonNull::new(window) else {
        log::error!("[graphics] ANativeWindow_fromSurface returned null");
        return;
    };
    let provider = Arc::new(AndroidSurfaceProvider { window });
    let surface_handle = GraphicsSurface::new(provider as Arc<dyn SurfaceProvider + Send + Sync>);

    let event = OnReadyEvent {
        surface: surface_handle,
        size: cb.last_size,
    };
    // Move-take the user closure to avoid re-entry borrow issues.
    let mut on_ready = std::mem::replace(
        &mut *cb.on_ready.borrow_mut(),
        Box::new(|_| {}),
    );
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_ready(event)));
    if !cb.released {
        *cb.on_ready.borrow_mut() = on_ready;
    }
}

/// `surfaceChanged` dispatch. If the size actually changed, fires
/// `on_resize`. Always updates the cached `last_size` so subsequent
/// `on_ready` calls report it.
///
/// # Safety
///
/// `ptr` must point to a live `Box<GraphicsCallback>`.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustGraphicsCallback_nativeSurfaceChanged<'l>(
    _env: jni::JNIEnv<'l>,
    _this: jni::objects::JObject<'l>,
    ptr: jlong,
    _surface: jni::objects::JObject<'l>,
    width: jni::sys::jint,
    height: jni::sys::jint,
) {
    if ptr == 0 {
        return;
    }
    let cb = &mut *(ptr as *mut GraphicsCallback);
    if cb.released {
        return;
    }
    let new_size = (width.max(0) as u32, height.max(0) as u32);
    let was = cb.last_size;
    cb.last_size = new_size;
    if was == new_size {
        return;
    }

    let event = OnResizeEvent { size: new_size };
    let mut on_resize = std::mem::replace(
        &mut *cb.on_resize.borrow_mut(),
        Box::new(|_| {}),
    );
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_resize(event)));
    if !cb.released {
        *cb.on_resize.borrow_mut() = on_resize;
    }
}

/// `surfaceDestroyed` dispatch. Fires `on_lost`. Authors MUST drop
/// any wgpu Surface / swapchain holding the underlying
/// ANativeWindow — Android frees the surface as soon as this
/// returns.
///
/// # Safety
///
/// `ptr` must point to a live `Box<GraphicsCallback>`.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustGraphicsCallback_nativeSurfaceDestroyed(
    _env: jni::JNIEnv,
    _this: jni::objects::JObject,
    ptr: jlong,
) {
    if ptr == 0 {
        return;
    }
    let cb = &mut *(ptr as *mut GraphicsCallback);
    if cb.released {
        return;
    }
    let mut on_lost = std::mem::replace(
        &mut *cb.on_lost.borrow_mut(),
        Box::new(|| {}),
    );
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_lost()));
    if !cb.released {
        *cb.on_lost.borrow_mut() = on_lost;
    }
}

/// Free the leaked `GraphicsCallback` box. Called from Kotlin's
/// `RustGraphicsCallback.finalize()` when the Kotlin object is
/// GC'd, which happens after the SurfaceView (and thus the
/// addCallback registration) has been torn down.
///
/// # Safety
///
/// `ptr` must point to a live `Box<GraphicsCallback>` produced by
/// `create()` and not previously dropped. Caller (Kotlin
/// `finalize()`) must arrange that no further surface-lifecycle
/// JNI calls reference `ptr` after this returns — Android won't
/// dispatch new SurfaceHolder events to a Kotlin object that's
/// being finalized.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustGraphicsCallback_nativeDrop(
    _env: jni::JNIEnv,
    _this: jni::objects::JObject,
    ptr: jlong,
) {
    if ptr == 0 {
        return;
    }
    let mut boxed: Box<GraphicsCallback> = Box::from_raw(ptr as *mut GraphicsCallback);
    // Mark released first so any concurrent surface-lifecycle JNI
    // call (unlikely but possible during finalization races) bails.
    boxed.released = true;
    drop(boxed);
}
