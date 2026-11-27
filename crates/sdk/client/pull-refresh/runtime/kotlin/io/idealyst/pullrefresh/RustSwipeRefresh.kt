package io.idealyst.pullrefresh

import android.content.Context
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import androidx.swiperefreshlayout.widget.SwipeRefreshLayout

/**
 * The `pull-refresh` SDK's `Refreshable` on Android: androidx
 * `SwipeRefreshLayout` around the wrapped child.
 *
 * ## Layout contract
 *
 * Rust registers this view as a layout-TRANSPARENT node
 * (`AndroidBackend::register_contents_view`), so the wrapped child keeps
 * the layout it would have unwrapped. That makes this view span its
 * nearest real ancestor's whole box, with the child's Taffy frame in its
 * `MarginLayoutParams`, as in every framework `FrameLayout`. Stock
 * `SwipeRefreshLayout` instead stretches its child over its own padding
 * box, so this class sets its padding to the child's frame every measure:
 * the stock measure and layout then place the child exactly at its frame,
 * and `clipToPadding` clips the spinner to the child's box, so it emerges
 * from the child's top edge as it does from a stock layout's.
 *
 * Only the spinner's offsets need moving with the child's top (stock
 * offsets are from the layout's top), and its horizontal centre with the
 * child's.
 *
 * ## Gestures
 *
 * - A touch that starts outside the child is refused, so the siblings this
 *   ancestor-sized view covers still get their taps.
 * - [canChildScrollUp] asks the child's tree, not only the child: the
 *   first view in pre-order that can scroll vertically at all decides. The
 *   stock check reads the direct child only, which for a `view` around a
 *   list always says "at the top".
 */
class RustSwipeRefresh(context: Context, private var handle: Long) : SwipeRefreshLayout(context) {

    /** The progress circle, added by the superclass constructor. It moves to
     *  the end of the child list whenever it is shown (`bringToFront`), so
     *  it is held by reference, not index. */
    private val circle: View? = if (childCount > 0) getChildAt(0) else null

    private val defaultStart = progressViewStartOffset
    private val defaultEnd = progressViewEndOffset
    private var spinnerTop = 0

    init {
        clipToPadding = true
        setOnRefreshListener {
            val h = handle
            if (h != 0L) nativeOnRefresh(h)
        }
    }

    /** The wrapped child: the first view that is not the circle. */
    private fun content(): View? {
        for (i in 0 until childCount) {
            val c = getChildAt(i)
            if (c !== circle) return c
        }
        return null
    }

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        val c = content()
        val lp = c?.layoutParams as? ViewGroup.MarginLayoutParams
        if (lp != null && lp.width >= 0 && lp.height >= 0) {
            val w = MeasureSpec.getSize(widthMeasureSpec)
            val h = MeasureSpec.getSize(heightMeasureSpec)
            val l = lp.leftMargin
            val t = lp.topMargin
            val r = (w - l - lp.width).coerceAtLeast(0)
            val b = (h - t - lp.height).coerceAtLeast(0)
            if (paddingLeft != l || paddingTop != t || paddingRight != r || paddingBottom != b) {
                setPadding(l, t, r, b)
            }
        }
        super.onMeasure(widthMeasureSpec, heightMeasureSpec)
    }

    override fun onLayout(changed: Boolean, left: Int, top: Int, right: Int, bottom: Int) {
        alignSpinner(paddingTop)
        super.onLayout(changed, left, top, right, bottom)
        // Stock layout centres the circle on this view; centre it on the
        // child, which is this view's padding box.
        val cv = circle ?: return
        val centre = paddingLeft + (width - paddingLeft - paddingRight) / 2
        val dx = centre - (cv.left + cv.width / 2)
        if (dx != 0) cv.offsetLeftAndRight(dx)
    }

    /** Shift the spinner's start and rest positions by the child's top,
     *  keeping the stock distances between them (and so the stock drag
     *  feel). `setProgressViewOffset` resets the circle and clears the
     *  refreshing flag, so a refresh in flight is shown again. */
    private fun alignSpinner(top: Int) {
        if (top == spinnerTop) return
        spinnerTop = top
        val wasRefreshing = isRefreshing
        // Stock rest position = end + start; a custom offset rests at `end`.
        setProgressViewOffset(false, defaultStart + top, defaultStart + defaultEnd + top)
        if (wasRefreshing) isRefreshing = true
    }

    override fun canChildScrollUp(): Boolean {
        for (i in 0 until childCount) {
            val c = getChildAt(i)
            if (c === circle) continue
            val decided = scrolledFromTop(c)
            if (decided != null) return decided
        }
        return false
    }

    /** `true`/`false` once a vertically scrollable view is found (is it
     *  away from its top?), `null` when this subtree has none. */
    private fun scrolledFromTop(v: View): Boolean? {
        if (v.canScrollVertically(-1)) return true
        if (v.canScrollVertically(1)) return false
        if (v is ViewGroup) {
            for (i in 0 until v.childCount) {
                val d = scrolledFromTop(v.getChildAt(i))
                if (d != null) return d
            }
        }
        return null
    }

    override fun dispatchTouchEvent(ev: MotionEvent): Boolean {
        if (ev.actionMasked == MotionEvent.ACTION_DOWN) {
            val inside = ev.x >= paddingLeft && ev.x < width - paddingRight &&
                ev.y >= paddingTop && ev.y < height - paddingBottom
            if (!inside) return false
        }
        return super.dispatchTouchEvent(ev)
    }

    /** The backend's `clear_children` must not take the circle with the
     *  content: the superclass keeps using it. */
    override fun removeAllViews() {
        for (i in childCount - 1 downTo 0) {
            if (getChildAt(i) !== circle) removeViewAt(i)
        }
    }

    /** `argb` when `has`, else the stock black arrow. */
    fun applyTint(has: Boolean, argb: Int) {
        setColorSchemeColors(if (has) argb else 0xFF000000.toInt())
    }

    /** Scope teardown: no pull reaches Rust after this. */
    fun detach() {
        handle = 0L
        setOnRefreshListener(null)
    }

    private external fun nativeOnRefresh(handle: Long)
}
