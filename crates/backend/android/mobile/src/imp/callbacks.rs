//! Owned holders for each kind of JNI-bridged callback. Each is leaked
//! at construction (`Box::into_raw`) so the JVM-side listener can
//! hold a `jlong` pointer to it; the matching JNI export dereferences
//! the pointer and invokes the inner closure.
//!
//! Lifetime: most of these leak for the activity's lifetime. The
//! state-callback case is special — see [`StateCallback`] — and
//! supports clearing the inner closure without freeing the box, since
//! late touch/focus events from Android's input dispatcher can fire
//! after the view detaches.

use jni::sys::jlong;
use std::cell::RefCell;
use std::rc::Rc;

/// `Button.onClick`. JVM-side `RustClickListener` holds the pointer
/// and dispatches via `nativeInvoke`.
///
/// Lifetime: leaked at the listener's construction. The Activity
/// owning the view tree lives for the app's lifetime in this demo,
/// so explicit drop-on-detach isn't wired. A production backend
/// would call back into Rust from the Kotlin listener's `finalize`
/// to drop these.
pub(crate) struct ClickCallback(pub(crate) Rc<dyn Fn()>);

/// `ScreenOptions.header_left.on_press` for drawer navigators.
/// `RustActionBarHelper` stores the pointer; the host Activity's
/// `onOptionsItemSelected` dispatches into Rust via `nativeInvoke`.
///
/// Lifetime: leaked for the lifetime of the screen. Each new screen
/// attach overwrites the slot — the previous box leaks. Bounded by
/// the number of screens the user navigates through, which for a
/// drawer-driven app is the size of the drawer item list. Could be
/// freed on the next attach if it grows, but the cost (~16 bytes per
/// screen) doesn't warrant the complexity today.
pub(crate) struct HeaderButtonCallback(pub(crate) Rc<dyn Fn()>);

/// Owned holder for the per-node state setter the framework hands
/// us in `attach_states`. JVM side keeps the raw pointer in
/// `RustStateListener` and passes it back via `nativeStateEvent` on
/// every touch/focus transition.
///
/// `inner` is a `RefCell<Option<...>>` so `on_node_unstyled` can
/// blank it out (drop the inner closure + the `Signal` setter it
/// captures) without freeing the box itself. Freeing the box is
/// unsafe: the Kotlin `RustStateListener` is wired as an
/// `OnTouchListener` + `OnFocusChangeListener` on the row view, and
/// Android's input dispatcher can deliver an event to a detached
/// View moments after we drop the per-item Scope. A freed pointer
/// here was the source of a SIGSEGV inside `nativeStateEvent`.
/// Leaking the wrapper box is the simple safe fix — at recycler
/// scale only a handful of holders are alive at once, so the bound
/// on accumulated boxes is small.
pub(crate) struct StateCallback {
    pub(crate) inner: RefCell<Option<Rc<dyn Fn(runtime_core::StateBits, bool)>>>,
}

/// `TextInput.on_change`. JVM-side `RustTextWatcher.afterTextChanged`
/// calls `nativeChanged(ptr, text)`.
pub(crate) struct TextChangeCallback(pub(crate) Rc<dyn Fn(String)>);

/// `TextInput.on_key_down` / `TextArea.on_key_down`. JVM-side
/// `RustKeyListener.onKey` calls `nativeKey(ptr, keyCode, metaState,
/// unicodeChar, selStart, selEnd)` and uses the returned bool as the
/// listener's "consumed" flag — true suppresses the platform default,
/// matching `KeyOutcome::PreventDefault`. The Rust handler closure
/// already takes a built `KeyEvent`, so this wrapper carries it
/// directly; the keycode → canonical-name mapping happens in the JNI
/// export itself.
pub(crate) struct KeyDownCallback(pub(crate) runtime_core::primitives::key::KeyDownHandler);

/// `Toggle.on_change`. JVM-side `RustToggleListener.onCheckedChanged`
/// calls `nativeChanged(ptr, checked)`.
pub(crate) struct ToggleChangeCallback(pub(crate) Rc<dyn Fn(bool)>);

/// `Slider.on_change`. The Kotlin `RustSliderListener` passes back
/// the SeekBar's integer progress; we convert to the user's
/// [min, max] f32 range before invoking the closure.
pub(crate) struct SliderChangeCallback {
    pub(crate) on_change: Rc<dyn Fn(f32)>,
    pub(crate) min: f32,
    pub(crate) max: f32,
    pub(crate) resolution: i32,
}

/// Portal dismiss callback. JVM-side `RustOverlayDismissListener`
/// (wired as `Dialog.OnCancelListener`) fires `nativeDismiss(ptr)`
/// on back-button press. `inner` is an `Option` so that
/// `release_portal` can clear it before the dialog finishes
/// dismissing — otherwise the framework-driven dismissal would
/// re-fire the user's `on_dismiss` and create a feedback loop
/// (user closure flips signal → surrounding `when` rebuilds →
/// `release_portal` runs → Android dismisses the dialog → cancel
/// listener fires → user closure runs again).
pub(crate) struct OverlayDismissCallback {
    pub(crate) inner: RefCell<Option<Rc<dyn Fn()>>>,
}

/// Leak a `Box<T>` and return its raw pointer as a `jlong`. Trivial
/// helper exposed for symmetry with the per-callback construction
/// sites scattered across the primitive modules.
pub(crate) fn leak<T>(value: T) -> jlong {
    Box::into_raw(Box::new(value)) as jlong
}

/// Owned holder for a framework-installed [`TouchHandler`].
/// `RustTouchListener` keeps a `jlong` pointing here; each
/// `MotionEvent` dispatch trampolines into Rust via
/// `nativeInvokeTouch`, which derefs this pointer and invokes the
/// inner handler.
///
/// `inner` is a `RefCell<Option<...>>` so a future
/// `release_touch_handler` path could swap or drop the handler
/// without freeing the box itself. Same posture as
/// [`StateCallback`] — late touch events from Android's input
/// dispatcher can fire after the View detaches, and a freed box
/// would SIGSEGV.
pub(crate) struct TouchCallback {
    pub(crate) inner: RefCell<Option<runtime_core::TouchHandler>>,
}
