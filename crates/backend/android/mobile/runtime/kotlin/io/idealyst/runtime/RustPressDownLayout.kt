package io.idealyst.runtime

import android.annotation.SuppressLint
import android.content.Context
import android.view.MotionEvent
import android.widget.FrameLayout

/**
 * The container behind a `pressable` with `PressActivation::Down`: the same
 * clickable `FrameLayout` a release-mode pressable is, with the same
 * `RustClickListener` as its `OnClickListener`, except a primary pointer
 * press activates it on `ACTION_DOWN` instead of on the release.
 *
 * Mechanism: an `onTouchEvent` override, not an `OnTouchListener`. A view
 * has one touch-listener slot and the style system's `RustStateListener`
 * (PRESSED state bit) takes it after creation; `View.dispatchTouchEvent`
 * runs that listener first and, since it returns `false`, then calls
 * `onTouchEvent` here, so both see every event.
 *
 * - Primary `ACTION_DOWN`: `performClick()` — the click listener fires
 *   once, with the click sound and `TYPE_VIEW_CLICKED` accessibility event
 *   a release click gets. The view shows pressed until the gesture ends.
 * - The rest of that gesture is consumed and fires nothing: `super`'s
 *   release-click, long-press and context-click never run. The down also
 *   calls `requestDisallowInterceptTouchEvent(true)`, so a scrolling
 *   ancestor does not take the gesture over: the action already fired, and
 *   letting the press turn into a scroll would move the control (and
 *   whatever it opened) out from under the pointer.
 * - A secondary-button mouse press is consumed and fires nothing, so it
 *   can't fall through to an ancestor pressable either.
 * - Keyboard (DPAD_CENTER / ENTER), TalkBack's ACTION_CLICK and
 *   `PressableOps::click` all reach `performClick` through `View`'s own
 *   paths, which this class does not touch: one activation each.
 * - The rest of a press that fired is REPORTED (`nativeHeldPress` →
 *   `runtime_shared::held_press`): each move, then the lift or cancel. The gesture is this view's until it ends, so something the
 *   press opened (a menu) can only follow the finger — light the row under
 *   it, pick the one it lifts over — if this view says where it is.
 * - Disabled (`setEnabled(false)`): defers to `super`, which consumes
 *   without clicking, like a disabled release-mode pressable.
 */
class RustPressDownLayout(context: Context) : FrameLayout(context) {
    /** A primary press is in progress (from its down to its up / cancel). */
    private var pressing = false

    // performClick IS called — on the down, which is the point of this class.
    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (!isEnabled) return super.onTouchEvent(event)
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                pressing = isPrimaryPress(event)
                if (pressing) {
                    parent?.requestDisallowInterceptTouchEvent(true)
                    isPressed = true
                    performClick()
                }
            }
            MotionEvent.ACTION_MOVE -> {
                if (pressing) reportHeld(event, HELD_MOVED)
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                if (pressing) {
                    isPressed = false
                    reportHeld(
                        event,
                        if (event.actionMasked == MotionEvent.ACTION_UP) HELD_RELEASED else HELD_CANCELLED,
                    )
                }
                pressing = false
            }
        }
        return true
    }

    /**
     * The pointer's position on screen, in physical px. The native side
     * converts it to the viewport dp the framework's handle rects are in
     * (it knows the Activity root's origin and density).
     */
    private fun reportHeld(event: MotionEvent, phase: Int) {
        getLocationOnScreen(screenLoc)
        nativeHeldPress(phase, screenLoc[0] + event.x, screenLoc[1] + event.y)
    }

    private val screenLoc = IntArray(2)

    private external fun nativeHeldPress(phase: Int, screenXPx: Float, screenYPx: Float)

    companion object {
        const val HELD_MOVED = 0
        const val HELD_RELEASED = 1
        const val HELD_CANCELLED = 2

        /**
         * Whether a touch-stream `ACTION_DOWN` is a primary press. Touch and
         * stylus contacts always are. A mouse press is primary when the
         * primary button is down, or when no button is reported at all: an
         * injected mouse tap (`adb shell input mouse tap`) carries
         * `buttonState == 0`, the same rule `app_pointer_policy` applies.
         */
        @JvmStatic
        fun isPrimaryPress(event: MotionEvent): Boolean {
            if (event.getToolType(0) != MotionEvent.TOOL_TYPE_MOUSE) return true
            val buttons = event.buttonState
            return buttons == 0 || (buttons and MotionEvent.BUTTON_PRIMARY) != 0
        }
    }
}
