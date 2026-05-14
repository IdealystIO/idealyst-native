package io.idealyst.runtime

import android.view.MotionEvent
import android.view.View

/**
 * `View.OnTouchListener` + `View.OnFocusChangeListener` that forwards
 * the corresponding state transitions into Rust via the
 * `nativeStateEvent` JNI trampoline.
 *
 * The Rust side hands us a raw pointer to a leaked
 * `Box<StateCallback>`. On each event we re-dispatch with that
 * pointer, an integer `bit` matching one of the `StateBits`
 * constants (`PRESSED = 2`, `FOCUSED = 4`), and an `on` boolean
 * encoded as 0/1.
 *
 * Mobile doesn't have a hover concept for touch input, so we don't
 * emit HOVERED — matches the cross-platform contract that hover
 * states are no-ops on Android/iOS.
 *
 * Touch handling: we return `false` from `onTouch` so the View's
 * own click handling (`OnClickListener`) still fires. Returning
 * `true` would consume the event and break clicks.
 */
class RustStateListener(private val nativePtr: Long) :
    View.OnTouchListener,
    View.OnFocusChangeListener {

    companion object {
        // Mirror StateBits in framework-core. PRESSED = 1 << 1,
        // FOCUSED = 1 << 2.
        private const val BIT_PRESSED = 2
        private const val BIT_FOCUSED = 4
    }

    override fun onTouch(v: View, event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> nativeStateEvent(nativePtr, BIT_PRESSED, 1)
            MotionEvent.ACTION_UP,
            MotionEvent.ACTION_CANCEL -> nativeStateEvent(nativePtr, BIT_PRESSED, 0)
        }
        // Don't consume — let the View's OnClickListener still fire.
        return false
    }

    override fun onFocusChange(v: View, hasFocus: Boolean) {
        nativeStateEvent(nativePtr, BIT_FOCUSED, if (hasFocus) 1 else 0)
    }

    private external fun nativeStateEvent(ptr: Long, bit: Int, on: Int)
}
