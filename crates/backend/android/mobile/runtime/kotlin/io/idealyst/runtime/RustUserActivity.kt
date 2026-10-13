package io.idealyst.runtime

import android.view.KeyEvent
import android.view.KeyboardShortcutGroup
import android.view.Menu
import android.view.MotionEvent
import android.view.View
import android.view.Window
import java.lang.ref.WeakReference

/**
 * App-wide "the person is using the app" source behind
 * `runtime_core::user_activity()` (Rust side: `imp/user_activity.rs`,
 * contract: `runtime_shared::user_activity`). Installed once at boot,
 * unconditionally, from `AndroidBackend::new`.
 *
 * Reports through the static [nativeReport] trampoline (no Rust pointer —
 * nothing to free, and a report after teardown only bumps a counter) on:
 *
 * - a touch-stream DOWN (`ACTION_DOWN` / `ACTION_POINTER_DOWN`) — finger,
 *   stylus or mouse button; a finger scroll/fling starts with one too;
 * - a key DOWN (hardware keyboard, D-pad, volume rocker the app sees);
 * - a generic `ACTION_SCROLL` (wheel / touchpad scroll) or
 *   `ACTION_BUTTON_PRESS` (a mouse button that isn't the primary one);
 * - a USER edit of a framework text input ([RustTextWatcher]) — soft
 *   keyboards commit text through the `InputConnection`, not key events,
 *   so typing on one never reaches `dispatchKeyEvent`.
 *
 * Moves, ups and hover are not reported: the down that started the gesture
 * already did, and filtering here keeps the per-move cost at zero JNI calls.
 * Rust throttles further (one tick per frame).
 *
 * # Why a `Window.Callback` wrapper
 *
 * Same reason as [RustGlobalPointerListener] (see `imp/pointer.rs`): the
 * window's callback (the Activity) is the one hook that sees every input
 * event of the window BEFORE any view can consume it. Every portal, modal,
 * popover and drawer is a view in this same window (`primitives/overlay.rs`),
 * so they are covered too. The one separate window the backend opens — the
 * screen-recorder private layer (`TYPE_APPLICATION_PANEL`, added through
 * `WindowManager.addView`, so it has no `Window` / `Window.Callback`) —
 * reports from its content view, [RustOverlayPassthrough].
 *
 * The wrapper OBSERVES ONLY: every member calls straight through to the
 * original callback and returns its result unchanged; nothing is consumed,
 * delayed or reordered. When [RustGlobalPointerListener] later wraps the
 * window on top of this one and consumes a mouse event (`PreventDefault`),
 * that event never reaches this wrapper, so the pointer wrapper reports it
 * itself ([onMotion]).
 *
 * UI thread only.
 */
object RustUserActivity {
    /** The window last wrapped — a second boot on the same Activity (hot
     *  restart) must not stack a second wrapper. Weak: the Activity owns it. */
    private var wrapped: WeakReference<Window>? = null

    /** Wrap the root's Activity window. No-op without an Activity window. */
    @JvmStatic
    fun install(root: View) {
        val w = RustGlobalPointerListener.findActivity(root.context)?.window ?: return
        if (wrapped?.get() === w) return
        val base = w.callback ?: return
        w.callback = RustActivityWindowCallback(base)
        wrapped = WeakReference(w)
    }

    /** Report if [event] starts an interaction (see the class docs). */
    @JvmStatic
    fun onMotion(event: MotionEvent) {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN,
            MotionEvent.ACTION_POINTER_DOWN,
            MotionEvent.ACTION_SCROLL,
            MotionEvent.ACTION_BUTTON_PRESS -> nativeReport()
        }
    }

    @JvmStatic
    fun onKey(event: KeyEvent) {
        if (event.action == KeyEvent.ACTION_DOWN) nativeReport()
    }

    /** The person edited a framework text input (IME commit or paste). */
    @JvmStatic
    fun onUserTextEdit() = nativeReport()

    /** `imp/jni_exports.rs` → `newcore::report_user_activity`. */
    @JvmStatic
    external fun nativeReport()
}

/**
 * Pass-through `Window.Callback` installed by [RustUserActivity.install]:
 * reports, then delegates to [base] and returns its answer untouched.
 */
internal class RustActivityWindowCallback(
    val base: Window.Callback,
) : Window.Callback by base {

    override fun dispatchTouchEvent(event: MotionEvent?): Boolean {
        if (event != null) RustUserActivity.onMotion(event)
        return base.dispatchTouchEvent(event)
    }

    override fun dispatchGenericMotionEvent(event: MotionEvent?): Boolean {
        if (event != null) RustUserActivity.onMotion(event)
        return base.dispatchGenericMotionEvent(event)
    }

    override fun dispatchKeyEvent(event: KeyEvent?): Boolean {
        if (event != null) RustUserActivity.onKey(event)
        return base.dispatchKeyEvent(event)
    }

    // Kotlin's `by base` does NOT forward Java DEFAULT interface methods —
    // the interface's no-op default would run instead of the Activity's
    // override, silently breaking pointer capture and keyboard-shortcut
    // publishing. These are the two default members of `Window.Callback`
    // (same list as `RustPointerWindowCallback`, checked with javap against
    // android-36).
    override fun onPointerCaptureChanged(hasCapture: Boolean) {
        base.onPointerCaptureChanged(hasCapture)
    }

    override fun onProvideKeyboardShortcuts(
        data: MutableList<KeyboardShortcutGroup>?,
        menu: Menu?,
        deviceId: Int,
    ) {
        base.onProvideKeyboardShortcuts(data, menu, deviceId)
    }
}
