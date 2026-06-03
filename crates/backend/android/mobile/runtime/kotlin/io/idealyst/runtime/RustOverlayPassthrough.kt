package io.idealyst.runtime

import android.content.Context
import android.graphics.Rect
import android.view.MotionEvent
import android.view.View
import android.widget.FrameLayout

/**
 * Content view for a `screen_recorder::PrivateLayer`'s overlay window.
 *
 * The overlay lives in a SEPARATE, full-screen `WindowManager` window so the
 * screen-capture path can exclude it from recordings. But a full-screen
 * touchable window swallows every touch, and Android has no per-region
 * passthrough for a full-screen window (the `OnComputeInternalInsetsListener`
 * touchable-region API is hidden/blocked on modern Android). So instead we
 * keep the window touchable and FORWARD any touch that doesn't land on one of
 * the overlay's own interactive children to [behind] — the main activity
 * window's decor view. Both windows are in the same process, so
 * `dispatchTouchEvent` reaches the app's real view tree (e.g. a drawable
 * canvas) underneath. The overlay's controls (a toolbar) still receive their
 * taps; everything else falls through to the app.
 *
 * Coordinates line up because both windows are full-screen at the screen
 * origin, so a `MotionEvent` in this window's space is valid in the main
 * window's space unchanged.
 *
 * This is the Android analog of iOS's `OverlayPassthroughView` hit-testing.
 */
class RustOverlayPassthrough(context: Context) : FrameLayout(context) {

    /** Main-window decor view to forward non-control touches into. */
    private var behind: View? = null

    /**
     * Whether the in-flight gesture is being forwarded to [behind]. Decided at
     * ACTION_DOWN by hit-testing the children, then held for the whole gesture
     * (Android delivers the full DOWN..UP stream to whoever took the DOWN).
     */
    private var forwarding = false
    private val hitRect = Rect()

    fun setBehind(view: View?) {
        behind = view
    }

    override fun dispatchTouchEvent(ev: MotionEvent): Boolean {
        if (ev.actionMasked == MotionEvent.ACTION_DOWN) {
            // New gesture: forward it unless it starts on an interactive child.
            forwarding = !isOverChild(ev.x, ev.y)
        }

        if (forwarding) {
            val target = behind
            val handled = target?.dispatchTouchEvent(ev) ?: false
            if (ev.actionMasked == MotionEvent.ACTION_UP ||
                ev.actionMasked == MotionEvent.ACTION_CANCEL
            ) {
                forwarding = false
            }
            return handled
        }

        return super.dispatchTouchEvent(ev)
    }

    /** Is (x, y) — in this view's coordinate space — over any visible child? */
    private fun isOverChild(x: Float, y: Float): Boolean {
        val xi = x.toInt()
        val yi = y.toInt()
        for (i in 0 until childCount) {
            val child = getChildAt(i)
            if (child.visibility != View.VISIBLE) continue
            child.getHitRect(hitRect)
            if (hitRect.contains(xi, yi)) return true
        }
        return false
    }
}
