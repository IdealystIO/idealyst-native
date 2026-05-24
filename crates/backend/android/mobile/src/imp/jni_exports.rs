//! `#[no_mangle]` JNI trampolines — one per Kotlin runtime class.
//! Each export downcasts its `jlong` pointer back to a leaked
//! callback box and invokes the inner closure, wrapped in
//! `catch_unwind` because Rust panics across the FFI boundary are UB.

use super::callbacks::{
    ClickCallback, HeaderButtonCallback, KeyDownCallback, OverlayDismissCallback,
    SliderChangeCallback, StateCallback, TextChangeCallback, ToggleChangeCallback, TouchCallback,
};
use jni::objects::{JObject, JValue};
use jni::sys::{jboolean, jfloat, jint, jlong};
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

/// `RustActionBarHelper.nativeInvoke` dispatches the home-button
/// (`header_left`) press into Rust. Same shape as the click listener
/// trampoline, distinct type so signatures don't blur at the Rust
/// callsite. The JVM signature is a *static* method (no `this`).
///
/// # Safety
///
/// `ptr` must have been produced by `Box::into_raw` on a
/// `Box<HeaderButtonCallback>` in `tab_drawer::apply_screen_options`
/// and must still be live.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustActionBarHelper_nativeInvoke(
    _env: JNIEnv,
    // Static method on `RustActionBarHelper`'s companion object — the
    // second JNI arg is the `Class` ref, not an instance.
    _class: JObject,
    ptr: jlong,
) {
    if ptr == 0 {
        return;
    }
    let cb = &*(ptr as *const HeaderButtonCallback);
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
    let bit = runtime_core::StateBits(bit as u8);
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
// Touch listener (raw-touch pipeline)
// ---------------------------------------------------------------------------

/// `RustTouchListener.onTouch` per-pointer dispatch. Receives every
/// field of the framework `TouchEvent` flattened across the JNI
/// boundary and returns a packed response:
///
/// ```text
///   bit 0 = consumed
///   bit 1 = claim
/// ```
///
/// # Safety
///
/// `ptr` must have been produced by `Box::into_raw` on a
/// `Box<TouchCallback>` from `primitives::touch::install` and must
/// still be live. The pointer is *not* freed here; late touch events
/// can arrive after the View detaches.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustTouchListener_nativeInvokeTouch(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
    id: jlong,
    phase: jint,
    x: jfloat,
    y: jfloat,
    win_x: jfloat,
    win_y: jfloat,
    timestamp_ns: jlong,
    force: jfloat,
) -> jint {
    if ptr == 0 {
        return 0;
    }
    use runtime_core::{TouchEvent, TouchId, TouchPhase, TouchPoint};
    let phase = match phase {
        0 => TouchPhase::Began,
        1 => TouchPhase::Moved,
        2 => TouchPhase::Ended,
        3 => TouchPhase::Cancelled,
        // Defensive: an unknown phase from the Kotlin side would be
        // a contract violation. Drop the event silently rather than
        // crash — easier to debug a missing handler call than a
        // SIGABRT from a JNI panic.
        _ => return 0,
    };
    // MotionEvent.getPressure returns 1.0 for non-pressure-sensitive
    // devices when a button is down. Treat that as "no information"
    // to match the iOS / web sentinel-filter behavior. Devices that
    // do report pressure (Surface Pen, 3D-touch-on-some-Android)
    // produce values in (0, 1).
    let force_opt = if force > 0.0 && force < 1.0 {
        Some(force)
    } else {
        None
    };
    let event = TouchEvent {
        id: TouchId(id as u64),
        phase,
        position: TouchPoint::new(x, y),
        window_position: TouchPoint::new(win_x, win_y),
        timestamp_ns: timestamp_ns as u64,
        force: force_opt,
    };
    let cb = &*(ptr as *const TouchCallback);
    let handler_opt = cb.inner.borrow().clone();
    let Some(handler) = handler_opt else {
        return 0;
    };
    // Catch unwinds across the JNI boundary — Rust panics into Java
    // are UB.
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(&event)))
        .unwrap_or(runtime_core::TouchResponse::IGNORED);
    let mut packed: jint = 0;
    if response.consumed {
        packed |= 0x1;
    }
    if response.claim {
        packed |= 0x2;
    }
    packed
}

/// Free a leaked `TouchCallback`. Currently unused (the Kotlin
/// `RustTouchListener.finalize` isn't wired) — exposed for symmetry
/// and future cleanup.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustTouchListener_nativeDrop(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
) {
    if ptr != 0 {
        drop(Box::from_raw(ptr as *mut TouchCallback));
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

/// `RustKeyListener.onKey` dispatch. Maps the Android keycode +
/// metaState + unicodeChar into the canonical `KeyEvent` shape
/// documented on `runtime_core::primitives::key`, invokes the
/// user's handler, and returns `true` for `KeyOutcome::PreventDefault`
/// so the EditText's default action is suppressed.
///
/// The keycode → string mapping uses the [Web `KeyboardEvent.key`
/// spec](https://developer.mozilla.org/en-US/docs/Web/API/UI_Events/Keyboard_event_key_values)
/// as its target vocabulary — same as the iOS and web backends do —
/// so a handler `if ev.key == "Tab"` works identically across all
/// three platforms. Unmapped keycodes fall back to the unicode
/// character if printable, else the empty string.
#[no_mangle]
pub unsafe extern "system" fn Java_io_idealyst_runtime_RustKeyListener_nativeKey(
    _env: JNIEnv,
    _this: JObject,
    ptr: jlong,
    key_code: jint,
    meta_state: jint,
    unicode_char: jint,
    sel_start: jint,
    sel_end: jint,
) -> jboolean {
    if ptr == 0 {
        return 0;
    }
    let cb = &*(ptr as *const KeyDownCallback);
    let key = android_key_name(key_code, unicode_char);
    // Android meta-state bitmask constants (see KeyEvent.java):
    // META_SHIFT_ON = 0x1, META_ALT_ON = 0x2, META_CTRL_ON = 0x1000,
    // META_META_ON = 0x10000. Bitmask check matches whether *either*
    // L/R variant of the modifier is pressed.
    let event = runtime_core::primitives::key::KeyEvent {
        key,
        shift: (meta_state & 0x1) != 0,
        ctrl: (meta_state & 0x1000) != 0,
        alt: (meta_state & 0x2) != 0,
        meta: (meta_state & 0x10000) != 0,
        selection_start: sel_start.max(0) as usize,
        selection_end: sel_end.max(0) as usize,
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (cb.0)(&event)))
        .unwrap_or(runtime_core::primitives::key::KeyOutcome::Default);
    match outcome {
        runtime_core::primitives::key::KeyOutcome::PreventDefault => 1,
        runtime_core::primitives::key::KeyOutcome::Default => 0,
    }
}

/// Map an Android keycode (plus a fallback unicode char for printable
/// keys) to the canonical web-style key name. Kept tight: only the
/// keys text-editor handlers typically reach for are named; everything
/// else falls back to the unicode char or an empty string.
fn android_key_name(key_code: jint, unicode_char: jint) -> String {
    // KeyEvent.KEYCODE_* constants. Numeric values copied from
    // Android source — they're stable ABI.
    match key_code {
        61 => "Tab".to_string(),
        66 | 160 => "Enter".to_string(),    // ENTER, NUMPAD_ENTER
        111 => "Escape".to_string(),
        67 => "Backspace".to_string(),       // KEYCODE_DEL is Android's name for Backspace
        112 => "Delete".to_string(),         // KEYCODE_FORWARD_DEL
        19 => "ArrowUp".to_string(),
        20 => "ArrowDown".to_string(),
        21 => "ArrowLeft".to_string(),
        22 => "ArrowRight".to_string(),
        122 => "Home".to_string(),
        123 => "End".to_string(),
        92 => "PageUp".to_string(),
        93 => "PageDown".to_string(),
        59 | 60 => "Shift".to_string(),      // SHIFT_LEFT, SHIFT_RIGHT
        57 | 58 => "Alt".to_string(),
        113 | 114 => "Control".to_string(),
        117 | 118 => "Meta".to_string(),
        _ => {
            // Printable: convert the unicode int to a Rust char. The
            // Android KeyEvent already accounts for modifier state in
            // `unicodeChar`, so shifted letters come through as
            // uppercase. Non-printable keys not in the named list
            // above fall through to "" — the handler can still see
            // the keydown via the selection_* fields and choose to
            // ignore.
            if unicode_char > 0 {
                std::char::from_u32(unicode_char as u32)
                    .map(|c| c.to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            }
        }
    }
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
/// `on_dismiss` closure (if still set). `release_portal` clears the
/// `inner` slot before tearing down the dialog, so framework-driven
/// dismissal doesn't re-fire and feedback-loop the open-state signal.
///
/// # Safety
///
/// `ptr` must point to a live `Box<OverlayDismissCallback>` produced
/// by `create_portal`. Stays valid until `release_portal` (which
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
/// portals' `PopupWindow.OnDismissListener` trampoline. Same
/// contract as the Dialog-flow dispatch above: invokes the user's
/// `on_dismiss` if `inner` is still set, no-ops if `release_portal`
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

pub(crate) type AndroidVirtCallbacks = runtime_core::VirtualizerCallbacks<jni::objects::GlobalRef>;

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
