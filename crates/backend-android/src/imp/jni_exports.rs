//! `#[no_mangle]` JNI trampolines — one per Kotlin runtime class.
//! Each export downcasts its `jlong` pointer back to a leaked
//! callback box and invokes the inner closure, wrapped in
//! `catch_unwind` because Rust panics across the FFI boundary are UB.

use super::callbacks::{
    ClickCallback, OverlayDismissCallback, SliderChangeCallback, StateCallback,
    TextChangeCallback, ToggleChangeCallback,
};
use jni::objects::{JObject, JValue};
use jni::sys::{jint, jlong};
use jni::JNIEnv;

// ---------------------------------------------------------------------------
// Click listener
// ---------------------------------------------------------------------------

/// `RustClickListener.onClick` calls `nativeInvoke(nativePtr)`, which
/// dispatches here.
///
/// # Safety
///
/// `ptr` must have been produced by `Box::into_raw` on a
/// `Box<ClickCallback>` in `create_button` and must still be live.
/// The pointer is *not* freed here — it stays valid for as long as
/// the listener object is alive.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustClickListener_nativeInvoke(
    _env: JNIEnv,
    // Instance method on RustClickListener; second JNI arg is `this`.
    // We don't need it — `ptr` carries everything.
    _this: JObject,
    ptr: jlong,
) {
    if ptr == 0 {
        return;
    }
    let cb = &*(ptr as *const ClickCallback);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (cb.0)()));
}

/// Free a leaked `ClickCallback`. Currently unused (see lifetime
/// note on `ClickCallback`); exposed so the Kotlin side can call it
/// from `RustClickListener.finalize()` once we wire that.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustClickListener_nativeDrop(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
) {
    if ptr != 0 {
        drop(Box::from_raw(ptr as *mut ClickCallback));
    }
}

// ---------------------------------------------------------------------------
// State listener (touch / focus)
// ---------------------------------------------------------------------------

/// `RustStateListener` forwards touch and focus events here. `bit` is
/// the integer value of the `StateBits` flag to flip (matches
/// `StateBits::PRESSED.0` etc.); `on` is the new value of that bit
/// (1 for entering the state, 0 for leaving).
///
/// # Safety
///
/// `ptr` must have been produced by `Box::into_raw` on a
/// `Box<StateCallback>` in `attach_states` and must still be live.
/// The pointer is *never* freed; `on_node_unstyled` blanks the inner
/// closure instead (see `StateCallback` doc).
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustStateListener_nativeStateEvent(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
    bit: jint,
    on: jint,
) {
    if ptr == 0 {
        return;
    }
    let cb = &*(ptr as *const StateCallback);
    let bit = framework_core::StateBits(bit as u8);
    let on = on != 0;
    // Clone the inner Rc out of the RefCell so we can release the
    // borrow before invoking — the callback flips a Signal which
    // might (transitively) re-enter Rust code that also reads
    // backend state. Holding the borrow across the call would risk a
    // re-entrant borrow_mut.
    let maybe_cb = cb.inner.borrow().clone();
    if let Some(setter) = maybe_cb {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| setter(bit, on)));
    }
}

// ---------------------------------------------------------------------------
// TextInput / Toggle / Slider change listeners
// ---------------------------------------------------------------------------

/// `RustTextWatcher.afterTextChanged` dispatch. Hands the new string
/// content to the Rust `on_change` closure.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustTextWatcher_nativeChanged<'l>(
    mut env: JNIEnv<'l>,
    _this: JObject<'l>,
    ptr: jlong,
    text: jni::objects::JString<'l>,
) {
    if ptr == 0 {
        return;
    }
    let s = env
        .get_string(&text)
        .ok()
        .map(|js| js.to_str().unwrap_or("").to_string())
        .unwrap_or_default();
    let cb = &*(ptr as *const TextChangeCallback);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (cb.0)(s)));
}

/// `RustToggleListener.onCheckedChanged` dispatch.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustToggleListener_nativeChanged(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
    checked: jint,
) {
    if ptr == 0 {
        return;
    }
    let cb = &*(ptr as *const ToggleChangeCallback);
    let v = checked != 0;
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (cb.0)(v)));
}

/// `RustSliderListener.onProgressChanged` dispatch. Converts the
/// SeekBar's integer progress back to the user's [min, max] f32 range
/// using the stashed callback metadata.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustSliderListener_nativeChanged(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
    progress: jint,
) {
    if ptr == 0 {
        return;
    }
    let cb = &*(ptr as *const SliderChangeCallback);
    // Map int progress in [0, resolution] back to f32 [min, max].
    let t = progress as f32 / cb.resolution as f32;
    let value = cb.min + t * (cb.max - cb.min);
    let on_change = cb.on_change.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_change(value)));
}

// ---------------------------------------------------------------------------
// Overlay dismiss
// ---------------------------------------------------------------------------

/// `RustOverlayDismissListener.onCancel` dispatch. Fires the user's
/// `on_dismiss` closure (if still set). `release_overlay` clears the
/// `inner` slot before tearing down the dialog, so framework-driven
/// dismissal doesn't re-fire and feedback-loop the open-state signal.
///
/// # Safety
///
/// `ptr` must point to a live `Box<OverlayDismissCallback>` produced
/// by `create_overlay`. Stays valid until `release_overlay` (which
/// blanks the inner closure but does NOT free the box — see the doc
/// on `OverlayDismissCallback` for why we leak rather than drop).
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustOverlayDismissListener_nativeDismiss(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
) {
    if ptr == 0 {
        return;
    }
    let cb = &*(ptr as *const OverlayDismissCallback);
    // Clone out of the RefCell so we release the borrow before
    // invoking the user closure — it flips a Signal which may
    // re-enter framework code that also reads backend state.
    let maybe_cb = cb.inner.borrow().clone();
    if let Some(dismiss) = maybe_cb {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dismiss()));
    }
}

/// Free a leaked `OverlayDismissCallback`. Currently unwired in the
/// demo (Activity outlives all overlays); exposed so a long-lived
/// app can call this from the Kotlin listener's `finalize()` to
/// release the box.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustOverlayDismissListener_nativeDrop(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
) {
    if ptr != 0 {
        drop(Box::from_raw(ptr as *mut OverlayDismissCallback));
    }
}

/// `RustPopupDismissListener.onDismiss` dispatch — element-anchored
/// overlays' `PopupWindow.OnDismissListener` trampoline. Same
/// contract as the Dialog-flow dispatch above: invokes the user's
/// `on_dismiss` if `inner` is still set, no-ops if `release_overlay`
/// has already blanked it.
///
/// # Safety
///
/// `ptr` must point to a live `Box<OverlayDismissCallback>`.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustPopupDismissListener_nativeDismiss(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
) {
    if ptr == 0 {
        return;
    }
    let cb = &*(ptr as *const OverlayDismissCallback);
    let maybe_cb = cb.inner.borrow().clone();
    if let Some(dismiss) = maybe_cb {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dismiss()));
    }
}

// ---------------------------------------------------------------------------
// RustListAdapter (RecyclerView virtualizer)
// ---------------------------------------------------------------------------
//
// The Kotlin adapter calls into Rust for every lifecycle event (item
// count, key, mount, release, measured size, drop). All five exports
// share a leaked `VirtualizerCallbacks` pointer; `nativeDrop` is the
// only one that frees the box.

pub(crate) type AndroidVirtCallbacks = framework_core::VirtualizerCallbacks<jni::objects::GlobalRef>;

/// Catch panics + downcast the pointer in one place.
unsafe fn with_callbacks<R>(
    ptr: jlong,
    f: impl FnOnce(&AndroidVirtCallbacks) -> R,
) -> Option<R> {
    if ptr == 0 {
        return None;
    }
    let cbs = &*(ptr as *const AndroidVirtCallbacks);
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(cbs))).ok()
}

#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustListAdapter_nativeItemCount(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
) -> jint {
    with_callbacks(ptr, |cbs| (cbs.item_count)() as jint).unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustListAdapter_nativeItemKey(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
    position: jint,
) -> jlong {
    with_callbacks(ptr, |cbs| (cbs.item_key)(position as usize) as jlong).unwrap_or(0)
}

/// Build the item subtree and return a `MountResult(view, scopeId)`.
/// Returning a custom Kotlin class from JNI is just a `new_object`
/// against the cached class with the right constructor signature.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustListAdapter_nativeMountItem<'l>(
    mut env: JNIEnv<'l>,
    _this: JObject<'l>,
    ptr: jlong,
    position: jint,
) -> jni::sys::jobject {
    let Some((view, scope_id)) =
        with_callbacks(ptr, |cbs| (cbs.mount_item)(position as usize))
    else {
        return std::ptr::null_mut();
    };
    let class = env
        .find_class("io/idealyst/runtime/RustListAdapter$MountResult")
        .unwrap();
    let result = env
        .new_object(
            &class,
            "(Landroid/view/View;J)V",
            &[
                JValue::Object(&view.as_obj()),
                JValue::Long(scope_id as jlong),
            ],
        )
        .unwrap();
    result.into_raw()
}

#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustListAdapter_nativeReleaseItem(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
    scope_id: jlong,
) {
    let _ = with_callbacks(ptr, |cbs| (cbs.release_item)(scope_id as u64));
}

#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustListAdapter_nativeSetMeasuredSize(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
    scope_id: jlong,
    size: f32,
) {
    let _ = with_callbacks(ptr, |cbs| (cbs.set_measured_size)(scope_id as u64, size));
}

/// Free the leaked `VirtualizerCallbacks` box. Called from Kotlin
/// when the adapter is detached or the activity tears down. Unused in
/// the current demo (the activity outlives the list); wired so
/// long-lived apps don't accumulate leaked callback boxes.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustListAdapter_nativeDrop(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
) {
    if ptr != 0 {
        drop(Box::from_raw(ptr as *mut AndroidVirtCallbacks));
    }
}
