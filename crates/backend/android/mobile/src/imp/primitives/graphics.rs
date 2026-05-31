//! `Element::Graphics` — `android.view.SurfaceView` exposed as a
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
use backend_android_core::helpers::apply_default_layout_params;
use crate::imp::{with_env, AndroidBackend};
use runtime_core::primitives::graphics::{
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
    /// Pending `ANativeWindow*` captured from `surfaceCreated` when
    /// we couldn't yet fire `on_ready` (size was 0×0 — see the
    /// comment on the create handler). Cleared once `on_ready`
    /// actually fires.
    pub(crate) pending_window: Option<NonNull<ndk_sys::ANativeWindow>>,
    /// True between the first `on_ready` fire and the next
    /// `surfaceDestroyed`. Gates `on_resize` (only fires after
    /// `on_ready`) and `on_lost` (only fires if we'd previously
    /// fired `on_ready`).
    pub(crate) ready_fired: bool,
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
        pending_window: None,
        ready_fired: false,
    };
    let ptr: jlong = leak(cb);

    with_env(|env| {
        // `TextureView` instead of `SurfaceView` so the rendered
        // surface composites NORMALLY inside the View tree —
        // sibling Views (a sidebar drawer scrim, a modal, a
        // floating button) overlay it as expected. A `SurfaceView`
        // with `setZOrderOnTop(true)` would paint the wgpu output
        // ABOVE every regular View in the window, including the
        // drawer-navigator's sidebar, which is the visible bug
        // ("simulator preview covers the sidebar"). The earlier
        // SurfaceView path used `setZOrderOnTop` to dodge a
        // separate problem — the View's own background drawable
        // occluding the punched hole when authors applied
        // `background: …` via `with_style`. TextureView has no
        // hole-punch model at all: the surface is a hardware-
        // accelerated texture rendered into the View's own draw
        // pass, so neither the View's background nor sibling Views
        // create occlusion artifacts. Cost: TextureView uses one
        // more GPU composition pass than SurfaceView and doesn't
        // get the dedicated overlay plane on hardware-accelerated
        // composites, but for a small embedded preview that's
        // immaterial.
        let class = env.find_class("android/view/TextureView").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();

        // Build the Kotlin listener wrapping our leaked pointer,
        // then attach it via `TextureView.setSurfaceTextureListener`.
        // `RustTextureListener` translates the four `SurfaceTextureListener`
        // methods into the same `nativeSurfaceCreated/Changed/Destroyed`
        // JNI exports `RustGraphicsCallback` uses — the Rust side's
        // callback path is unchanged.
        let cb_class = env
            .find_class("io/idealyst/runtime/RustTextureListener")
            .unwrap();
        let cb_obj = env
            .new_object(&cb_class, "(J)V", &[JValue::Long(ptr)])
            .unwrap();
        env.call_method(
            &local,
            "setSurfaceTextureListener",
            "(Landroid/view/TextureView$SurfaceTextureListener;)V",
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
/// Java `Surface` and either:
///
/// - fires `on_ready` immediately if we already know a non-zero
///   drawable size (uncommon — would require a `surfaceChanged`
///   before `surfaceCreated`, which Android doesn't do in practice
///   but the lifecycle doesn't forbid);
/// - OR stashes the surface in `pending_window` and waits for the
///   first `surfaceChanged` to arrive before firing `on_ready`.
///
/// The deferred-fire path is the normal one. Firing `on_ready` with
/// `size = (0, 0)` is technically allowed by the framework's API
/// (authors get `event.size.max(1)` on the call site) but produces
/// a single-pixel wgpu swapchain whose `on_resize` may be missed if
/// it arrives before the renderer's async init completes — the
/// renderer then runs at 1×1 forever, which presents as a wildly
/// distorted image stretched to fill the SurfaceView.
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
    // the matching release happens when the SurfaceProvider drops
    // (or here, if we stash the window and `surfaceDestroyed`
    // arrives before we ever fire `on_ready`).
    let raw = env.get_raw();
    let surface_ptr = surface.as_raw();
    let window = ndk_sys::ANativeWindow_fromSurface(raw.cast(), surface_ptr.cast());
    let Some(window) = NonNull::new(window) else {
        log::error!("[graphics] ANativeWindow_fromSurface returned null");
        return;
    };

    if cb.last_size.0 > 0 && cb.last_size.1 > 0 {
        // Size is already known — fire on_ready immediately. Wrap
        // the window in the provider so its `Drop` will release
        // the ref count when wgpu eventually drops the surface.
        fire_on_ready(cb, window);
    } else {
        // Defer. Stash the window; `surfaceChanged` will fire
        // `on_ready` when the real size arrives.
        cb.pending_window = Some(window);
    }
}

/// `surfaceChanged` dispatch. If `on_ready` was deferred from
/// `surfaceCreated`, fires it here with the now-known size.
/// Otherwise (already fired): fires `on_resize` when the size
/// actually changes.
///
/// Always updates `last_size` first so a subsequent `surfaceCreated`
/// for the same surface would see it.
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

    // Deferred `on_ready` path — fire it now with the real size.
    if let Some(window) = cb.pending_window.take() {
        if new_size.0 > 0 && new_size.1 > 0 {
            fire_on_ready(cb, window);
        } else {
            // Still 0×0 (shouldn't normally happen — Android usually
            // delivers a real size here). Re-stash and wait.
            cb.pending_window = Some(window);
        }
        return;
    }

    // Normal resize path. Don't dispatch if the size didn't actually
    // change, and don't dispatch at all unless `on_ready` has fired
    // (authors expect resize to follow ready, never precede it).
    if !cb.ready_fired || was == new_size {
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

/// `surfaceDestroyed` dispatch. Fires `on_lost` only if `on_ready`
/// had previously fired (so author code that does `if on_ready:
/// build_renderer; if on_lost: drop_renderer` stays balanced).
///
/// If we'd stashed a `pending_window` (surfaceCreated arrived but
/// `surfaceChanged` never delivered a real size before destruction),
/// release it here so the ANativeWindow ref count is paired.
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

    // Release any never-handed-off pending window. If we had handed
    // it to the user (via on_ready), the wgpu Surface they built
    // around it owns the ref count and its Drop releases — we MUST
    // NOT double-release here.
    if let Some(window) = cb.pending_window.take() {
        ndk_sys::ANativeWindow_release(window.as_ptr());
    }

    // Only fire on_lost if on_ready had fired; otherwise the user's
    // closure never had a renderer to release and would be confused
    // by an unpaired on_lost.
    if !cb.ready_fired {
        return;
    }
    cb.ready_fired = false;

    let mut on_lost = std::mem::replace(
        &mut *cb.on_lost.borrow_mut(),
        Box::new(|| {}),
    );
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_lost()));
    if !cb.released {
        *cb.on_lost.borrow_mut() = on_lost;
    }
}

/// Build the `GraphicsSurface` wrapper around the given window and
/// fire `on_ready`. Shared between the immediate path (size already
/// known at `surfaceCreated`) and the deferred path (size arrived
/// via `surfaceChanged` after `surfaceCreated`).
///
/// # Safety
///
/// `cb` and `window` must be valid; caller ensures `cb.released`
/// has been checked.
unsafe fn fire_on_ready(
    cb: &mut GraphicsCallback,
    window: NonNull<ndk_sys::ANativeWindow>,
) {
    let provider = Arc::new(AndroidSurfaceProvider { window });
    let surface_handle = GraphicsSurface::new(provider as Arc<dyn SurfaceProvider + Send + Sync>);

    cb.ready_fired = true;
    let event = OnReadyEvent {
        surface: surface_handle,
        size: cb.last_size,
    };
    let mut on_ready = std::mem::replace(
        &mut *cb.on_ready.borrow_mut(),
        Box::new(|_| {}),
    );
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_ready(event)));
    if !cb.released {
        *cb.on_ready.borrow_mut() = on_ready;
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

// =====================================================================
// JNI exports for the TextureView listener path
// =====================================================================
//
// `TextureView.SurfaceTextureListener` lives on a different Kotlin
// class (`RustTextureListener`), so the JNI symbol mangler emits
// distinct `Java_..._RustTextureListener_native…` names. The
// underlying lifecycle is identical to the `SurfaceView` path —
// these are thin forwards to the same `GraphicsCallback` helpers.
// Authors don't get a choice between the two listeners: this file's
// `create(...)` picks `TextureView` for the Android Graphics
// primitive so embedded previews composite normally with the View
// tree (sidebar overlays, drawer scrim, modals).

#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustTextureListener_nativeSurfaceCreated<'l>(
    env: jni::JNIEnv<'l>,
    this: jni::objects::JObject<'l>,
    ptr: jlong,
    surface: jni::objects::JObject<'l>,
) {
    Java_io_idealyst_runtime_RustGraphicsCallback_nativeSurfaceCreated(env, this, ptr, surface);
}

#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustTextureListener_nativeSurfaceChanged<'l>(
    env: jni::JNIEnv<'l>,
    this: jni::objects::JObject<'l>,
    ptr: jlong,
    surface: jni::objects::JObject<'l>,
    width: i32,
    height: i32,
) {
    Java_io_idealyst_runtime_RustGraphicsCallback_nativeSurfaceChanged(env, this, ptr, surface, width, height);
}

#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustTextureListener_nativeSurfaceDestroyed(
    env: jni::JNIEnv,
    this: jni::objects::JObject,
    ptr: jlong,
) {
    Java_io_idealyst_runtime_RustGraphicsCallback_nativeSurfaceDestroyed(env, this, ptr);
}

#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustTextureListener_nativeDrop(
    env: jni::JNIEnv,
    this: jni::objects::JObject,
    ptr: jlong,
) {
    Java_io_idealyst_runtime_RustGraphicsCallback_nativeDrop(env, this, ptr);
}
