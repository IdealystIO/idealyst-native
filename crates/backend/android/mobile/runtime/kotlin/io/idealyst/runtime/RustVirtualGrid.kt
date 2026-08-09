package io.idealyst.runtime

import android.content.Context
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.view.ViewConfiguration
import android.widget.OverScroller
import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * Two-axis scrolling container for the framework's `virtual_grid`.
 *
 * ## Why not RecyclerView
 *
 * The 1-D virtualizer uses `RecyclerView` because a stock
 * `LinearLayoutManager` / `GridLayoutManager` gives it recycling for
 * free. Neither scrolls two axes: `canScrollHorizontally` and
 * `canScrollVertically` are mutually exclusive in every stock manager,
 * so a grid would need a custom `LayoutManager` re-implementing
 * `onLayoutChildren`, both `scroll*By` methods, recycling and saved
 * state — several hundred lines re-deriving a visible-rect search that
 * `runtime_shared::primitives::virtual_grid::GridMetrics` already
 * performs for every other backend.
 *
 * So this is a plain `ViewGroup` that scrolls itself, and Rust decides
 * which cells exist and where. Android diverges in MECHANISM
 * (`scrollTo` + `OverScroller` fling vs a `<div>`'s native overflow);
 * the observable behavior converges (CLAUDE.md §7).
 *
 * ## Layout contract
 *
 * Children are positioned in CONTENT space by the Rust side, which
 * calls `setCellFrame` after adding each cell. `onLayout` then applies
 * those stored frames verbatim — this view never measures or arranges
 * children itself, because their geometry comes from the grid's column
 * and row metrics, not from Android's measure pass.
 *
 * Scroll position is applied with `scrollTo`, which offsets the whole
 * child canvas — so a cell's stored frame stays in content space and
 * never has to be rewritten as the user scrolls.
 */
class RustVirtualGrid(context: Context, private val nativePtr: Long) : ViewGroup(context) {

    private val scroller = OverScroller(context)
    private val touchSlop = ViewConfiguration.get(context).scaledTouchSlop
    private val minFling = ViewConfiguration.get(context).scaledMinimumFlingVelocity
    private val maxFling = ViewConfiguration.get(context).scaledMaximumFlingVelocity

    private var lastTouchX = 0f
    private var lastTouchY = 0f
    private var dragging = false
    private var velocityTracker: android.view.VelocityTracker? = null

    /** Total scrollable content extent, in device pixels. Set by Rust
     *  from the grid metrics; bounds every scroll clamp below. */
    private var contentWidth = 0
    private var contentHeight = 0

    /** Per-child content-space frames, parallel to the child list. */
    private val frames = HashMap<View, IntArray>()

    init {
        // Children are absolutely positioned across the full content
        // extent, so the default child-clipping is what keeps the
        // off-window cells from painting outside the viewport.
        clipToPadding = false
        isChildrenDrawingOrderEnabled = false
    }

    fun setContentSize(w: Int, h: Int) {
        if (w == contentWidth && h == contentHeight) return
        contentWidth = w
        contentHeight = h
        // A shrink can leave the current offset past the new content
        // end; clamp before the next draw or the grid shows a blank
        // region it can't scroll back from.
        clampScroll()
        requestLayout()
    }

    /** Record a cell's content-space frame. Called by Rust right after
     *  `addView`, and again if metrics change under a mounted cell. */
    fun setCellFrame(child: View, x: Int, y: Int, w: Int, h: Int) {
        frames[child] = intArrayOf(x, y, w, h)
        child.measure(
            MeasureSpec.makeMeasureSpec(w, MeasureSpec.EXACTLY),
            MeasureSpec.makeMeasureSpec(h, MeasureSpec.EXACTLY),
        )
        child.layout(x, y, x + w, y + h)
    }

    override fun onViewRemoved(child: View) {
        super.onViewRemoved(child)
        frames.remove(child)
    }

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        // The grid fills whatever box its parent gives it — its own
        // size is the VIEWPORT, never the content extent. Measuring to
        // the content would make a year-long schedule an infinitely
        // wide view and defeat the windowing.
        setMeasuredDimension(
            getDefaultSize(suggestedMinimumWidth, widthMeasureSpec),
            getDefaultSize(suggestedMinimumHeight, heightMeasureSpec),
        )
        for (i in 0 until childCount) {
            val child = getChildAt(i)
            val f = frames[child] ?: continue
            child.measure(
                MeasureSpec.makeMeasureSpec(f[2], MeasureSpec.EXACTLY),
                MeasureSpec.makeMeasureSpec(f[3], MeasureSpec.EXACTLY),
            )
        }
    }

    override fun onLayout(changed: Boolean, l: Int, t: Int, r: Int, b: Int) {
        for (i in 0 until childCount) {
            val child = getChildAt(i)
            val f = frames[child] ?: continue
            child.layout(f[0], f[1], f[0] + f[2], f[1] + f[3])
        }
        // A size change means a new viewport, so the visible window
        // may have grown or shrunk on either axis.
        if (changed) nativeViewportChanged(nativePtr, width, height)
    }

    private fun maxScrollX() = (contentWidth - width).coerceAtLeast(0)
    private fun maxScrollY() = (contentHeight - height).coerceAtLeast(0)

    private fun clampScroll() {
        val x = scrollX.coerceIn(0, maxScrollX())
        val y = scrollY.coerceIn(0, maxScrollY())
        if (x != scrollX || y != scrollY) scrollTo(x, y)
    }

    override fun onInterceptTouchEvent(ev: MotionEvent): Boolean {
        // Claim the gesture only once it exceeds the slop on EITHER
        // axis, so a tap on a cell still reaches the cell. Intercepting
        // on DOWN would make every cell unpressable.
        when (ev.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                lastTouchX = ev.x
                lastTouchY = ev.y
                dragging = false
                scroller.forceFinished(true)
            }
            MotionEvent.ACTION_MOVE -> {
                if (abs(ev.x - lastTouchX) > touchSlop || abs(ev.y - lastTouchY) > touchSlop) {
                    dragging = true
                    // Stop an ancestor scroller from also acting on
                    // this gesture — the grid owns both axes.
                    parent?.requestDisallowInterceptTouchEvent(true)
                    return true
                }
            }
        }
        return false
    }

    override fun onTouchEvent(ev: MotionEvent): Boolean {
        val tracker = velocityTracker ?: android.view.VelocityTracker.obtain().also {
            velocityTracker = it
        }
        tracker.addMovement(ev)

        when (ev.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                lastTouchX = ev.x
                lastTouchY = ev.y
                scroller.forceFinished(true)
                return true
            }
            MotionEvent.ACTION_MOVE -> {
                val dx = (lastTouchX - ev.x).roundToInt()
                val dy = (lastTouchY - ev.y).roundToInt()
                lastTouchX = ev.x
                lastTouchY = ev.y
                if (dx != 0 || dy != 0) {
                    scrollTo(
                        (scrollX + dx).coerceIn(0, maxScrollX()),
                        (scrollY + dy).coerceIn(0, maxScrollY()),
                    )
                }
                return true
            }
            MotionEvent.ACTION_UP -> {
                tracker.computeCurrentVelocity(1000, maxFling.toFloat())
                val vx = -tracker.xVelocity
                val vy = -tracker.yVelocity
                if (abs(vx) > minFling || abs(vy) > minFling) {
                    scroller.fling(
                        scrollX, scrollY,
                        vx.roundToInt(), vy.roundToInt(),
                        0, maxScrollX(), 0, maxScrollY(),
                    )
                    postInvalidateOnAnimation()
                }
                releaseTracker()
                dragging = false
                return true
            }
            MotionEvent.ACTION_CANCEL -> {
                releaseTracker()
                dragging = false
                return true
            }
        }
        return super.onTouchEvent(ev)
    }

    private fun releaseTracker() {
        velocityTracker?.recycle()
        velocityTracker = null
    }

    override fun computeScroll() {
        if (scroller.computeScrollOffset()) {
            scrollTo(scroller.currX, scroller.currY)
            postInvalidateOnAnimation()
        }
    }

    override fun onScrollChanged(l: Int, t: Int, oldl: Int, oldt: Int) {
        super.onScrollChanged(l, t, oldl, oldt)
        // Rust re-windows here: this fires for drags, flings AND
        // programmatic `scrollTo`, so there is one notification path
        // rather than three.
        val density = resources.displayMetrics.density
        if (density <= 0f) return
        nativeOnScroll(nativePtr, l / density, t / density)
    }

    /** Programmatic scroll from `VirtualGridHandle`. Input is dp;
     *  `scrollTo` takes device pixels. */
    fun scrollToDp(x: Float, y: Float) {
        val density = resources.displayMetrics.density
        if (density <= 0f) return
        scroller.forceFinished(true)
        scrollTo(
            (x * density).roundToInt().coerceIn(0, maxScrollX()),
            (y * density).roundToInt().coerceIn(0, maxScrollY()),
        )
    }

    private external fun nativeOnScroll(ptr: Long, x: Float, y: Float)
    private external fun nativeViewportChanged(ptr: Long, widthPx: Int, heightPx: Int)
}
