package io.idealyst.runtime

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.os.Build
import android.view.KeyboardShortcutGroup
import android.view.Menu
import android.view.MotionEvent
import android.view.PointerIcon
import android.view.View
import android.view.ViewGroup
import android.view.ViewTreeObserver
import android.view.Window

/**
 * App-level mouse source attached by the Rust backend's `set_pointer_sink`
 * (`imp/pointer.rs` — read its module docs for WHY each hook was chosen).
 *
 * - [RustPointerWindowCallback] wraps the Activity window's
 *   `Window.Callback`: the only hook the backend can reach that sees every
 *   touch-stream and generic `MotionEvent` of the window BEFORE any view
 *   (children consume first under `ViewGroup.dispatchTouchEvent`, so a
 *   root-view listener would miss events over buttons, scroll views and text
 *   inputs). Each event goes to `nativePointerMotion` (Rust filters out
 *   finger touches and translates); `true` = consume.
 * - Pointer lock = pointer capture (API 26+): [setLock] focuses the root,
 *   sets an `OnCapturedPointerListener` on it (captured events follow the
 *   focus chain, not `Window.Callback`) and calls `requestPointerCapture()`.
 *   The grant / loss arrives via `onPointerCaptureChanged` →
 *   `nativePointerLockChanged`. Focus moving off the root while captured
 *   releases the capture (the new focus owner would swallow the events).
 * - Cursor hiding (API 24+): a top-most, non-interactive "shield" view with
 *   `PointerIcon.TYPE_NULL` in the `DecorView` wins pointer-icon resolution
 *   everywhere in the window (children resolve first, so an icon on the root
 *   would lose to a text input's I-beam) while every event falls through it.
 * - Window focus loss → `nativePointerFocusLost` (held buttons' releases
 *   won't arrive).
 *
 * Lifecycle: Rust constructs it with the pointer to a boxed sink, calls
 * [attach] (false = unsupported, Rust frees the box) and [detach] BEFORE
 * freeing it; [detach] zeroes [nativePtr] so a queued callback is a no-op.
 * UI thread only. The `external` signatures must match
 * `imp/jni_exports.rs` exactly (JNI binds by name).
 */
class RustGlobalPointerListener(private var nativePtr: Long) :
    ViewTreeObserver.OnGlobalFocusChangeListener {

    private var root: View? = null
    private var window: Window? = null
    private var callback: RustPointerWindowCallback? = null
    private var observer: ViewTreeObserver? = null
    private var shield: View? = null
    private val loc = IntArray(2)

    fun attach(root: View): Boolean {
        // ACTION_BUTTON_PRESS / PointerIcon / the Window.Callback API-24
        // members: below Nougat there is no app-level mouse source.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) return false
        val activity = findActivity(root.context) ?: return false
        val w = activity.window ?: return false
        val base = w.callback ?: return false
        this.root = root
        val cb = RustPointerWindowCallback(base, this)
        w.callback = cb
        window = w
        callback = cb
        val vto = root.viewTreeObserver
        vto.addOnGlobalFocusChangeListener(this)
        observer = vto
        return true
    }

    fun detach() {
        // Release capture / restore the cursor while the pointer is still
        // live is pointless (no one to report to) — zero it first so the
        // asynchronous capture-change callback is a no-op.
        nativePtr = 0L
        setLock(false)
        setCursorHidden(false)
        val r = root
        root = null
        callback?.let { cb ->
            cb.owner = null
            // Only unwrap if nobody wrapped the window after us; otherwise
            // our wrapper stays as an inert pass-through (owner == null).
            if (window?.callback === cb) window?.callback = cb.base
        }
        callback = null
        window = null
        if (r != null) {
            for (vto in listOfNotNull(observer, r.viewTreeObserver)) {
                if (vto.isAlive) vto.removeOnGlobalFocusChangeListener(this)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                r.setOnCapturedPointerListener(null)
            }
        }
        observer = null
    }

    fun setLock(locked: Boolean) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return // no capture: never reports true
        val r = root ?: return
        if (locked) {
            // Captured events are routed to the FOCUSED view only.
            r.isFocusable = true
            r.isFocusableInTouchMode = true
            if (!r.isFocused) r.requestFocus()
            r.setOnCapturedPointerListener { _, event -> onCaptured(event) }
            if (r.hasPointerCapture()) {
                // Already held (e.g. re-request after a sink swap): no
                // change callback will come, so report it now.
                val ptr = nativePtr
                if (ptr != 0L) nativePointerLockChanged(ptr, true)
            } else {
                // Ignored by the system while the window lacks focus — then
                // nothing is reported (a refused request).
                r.requestPointerCapture()
            }
        } else if (r.hasPointerCapture()) {
            r.releasePointerCapture()
        }
    }

    fun setCursorHidden(hidden: Boolean) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) return
        if (!hidden) {
            shield?.let { (it.parent as? ViewGroup)?.removeView(it) }
            shield = null
            return
        }
        if (shield != null) return
        val r = root ?: return
        val decor = r.rootView as? ViewGroup ?: return
        val v = View(r.context)
        v.pointerIcon = PointerIcon.getSystemIcon(r.context, PointerIcon.TYPE_NULL)
        v.isClickable = false
        v.isLongClickable = false
        v.isFocusable = false
        v.importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO_HIDE_DESCENDANTS
        decor.addView(
            v,
            ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT),
        )
        shield = v
    }

    /** From the window-callback wrapper. Window coordinates → root-relative px. */
    internal fun onMotion(event: MotionEvent, touchStream: Boolean): Boolean {
        val ptr = nativePtr
        val r = root
        if (ptr == 0L || r == null || event.pointerCount == 0) return false
        r.getLocationInWindow(loc)
        return nativePointerMotion(
            ptr,
            event.actionMasked,
            event.buttonState,
            event.source,
            event.getToolType(0),
            event.x - loc[0],
            event.y - loc[1],
            event.getAxisValue(MotionEvent.AXIS_VSCROLL),
            event.getAxisValue(MotionEvent.AXIS_HSCROLL),
            event.metaState,
            r.resources.displayMetrics.density,
            false,
            touchStream,
        )
    }

    /** Captured (locked) events: getX/getY are relative mouse counts. */
    private fun onCaptured(event: MotionEvent): Boolean {
        val ptr = nativePtr
        val r = root
        if (ptr == 0L || r == null || event.pointerCount == 0) return true
        val touchStream = when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP,
            MotionEvent.ACTION_MOVE, MotionEvent.ACTION_CANCEL -> true
            else -> false
        }
        nativePointerMotion(
            ptr,
            event.actionMasked,
            event.buttonState,
            event.source,
            event.getToolType(0),
            event.x,
            event.y,
            event.getAxisValue(MotionEvent.AXIS_VSCROLL),
            event.getAxisValue(MotionEvent.AXIS_HSCROLL),
            event.metaState,
            r.resources.displayMetrics.density,
            true,
            touchStream,
        )
        // A captured stream never reaches views; always handled.
        return true
    }

    internal fun onWindowFocus(hasFocus: Boolean) {
        val ptr = nativePtr
        if (!hasFocus && ptr != 0L) nativePointerFocusLost(ptr)
    }

    internal fun onCaptureChanged(hasCapture: Boolean) {
        val ptr = nativePtr
        if (ptr != 0L) nativePointerLockChanged(ptr, hasCapture)
    }

    override fun onGlobalFocusChanged(oldFocus: View?, newFocus: View?) {
        val r = root ?: return
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        if (oldFocus === r && newFocus !== r && r.hasPointerCapture()) {
            r.releasePointerCapture()
        }
    }

    private external fun nativePointerMotion(
        ptr: Long,
        action: Int,
        buttonState: Int,
        source: Int,
        toolType: Int,
        x: Float,
        y: Float,
        vscroll: Float,
        hscroll: Float,
        metaState: Int,
        density: Float,
        captured: Boolean,
        touchStream: Boolean,
    ): Boolean

    private external fun nativePointerLockChanged(ptr: Long, locked: Boolean)

    private external fun nativePointerFocusLost(ptr: Long)

    companion object {
        fun findActivity(context: Context?): Activity? {
            var c = context
            while (c != null) {
                if (c is Activity) return c
                c = (c as? ContextWrapper)?.baseContext
            }
            return null
        }
    }
}

/**
 * Delegating `Window.Callback` installed by [RustGlobalPointerListener]:
 * everything forwards to [base] (the Activity) except the four members the
 * pointer source observes. [owner] is nulled on detach, turning the wrapper
 * into a pure pass-through if it can't be unwrapped.
 */
internal class RustPointerWindowCallback(
    val base: Window.Callback,
    var owner: RustGlobalPointerListener?,
) : Window.Callback by base {

    override fun dispatchTouchEvent(event: MotionEvent?): Boolean {
        val o = owner
        if (o != null && event != null && o.onMotion(event, true)) return true
        return base.dispatchTouchEvent(event)
    }

    override fun dispatchGenericMotionEvent(event: MotionEvent?): Boolean {
        val o = owner
        if (o != null && event != null && o.onMotion(event, false)) return true
        return base.dispatchGenericMotionEvent(event)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        base.onWindowFocusChanged(hasFocus)
        owner?.onWindowFocus(hasFocus)
    }

    override fun onPointerCaptureChanged(hasCapture: Boolean) {
        base.onPointerCaptureChanged(hasCapture)
        owner?.onCaptureChanged(hasCapture)
    }

    // Kotlin's `by base` does NOT forward Java DEFAULT interface methods —
    // the interface default (a no-op) would run instead of the Activity's
    // override. `onPointerCaptureChanged` is handled above; this is the only
    // other default member of `Window.Callback` (checked with javap against
    // android-36), so the Activity keeps publishing its keyboard shortcuts.
    override fun onProvideKeyboardShortcuts(
        data: MutableList<KeyboardShortcutGroup>?,
        menu: Menu?,
        deviceId: Int,
    ) {
        base.onProvideKeyboardShortcuts(data, menu, deviceId)
    }
}
