package io.idealyst.runtime

import android.content.Context
import android.graphics.Canvas
import android.view.MotionEvent
import android.view.VelocityTracker
import android.view.View
import android.view.ViewConfiguration
import android.view.ViewGroup
import android.widget.EdgeEffect
import android.widget.FrameLayout
import android.widget.OverScroller
import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * The outer view of a `scroll_view(axis = ScrollAxis::Both)`: one
 * scroller that pans its single content child on both axes at once.
 *
 * ## Why not ScrollView inside HorizontalScrollView
 *
 * The stock scrollers each own one axis and claim a drag as soon as it
 * crosses the slop on THAT axis, so a diagonal pan moves only whichever
 * one claimed it. And `Position::Sticky` pins against the nearest
 * scroll ancestor: a `top` + `left` corner cell would see one of the
 * two scroll offsets, never both. One view whose `scrollX`/`scrollY`
 * are both live gives the sticky walk, the `OnScrollChangeListener`
 * (`on_scroll`) and `View.scrollTo` (scroll handle, robot) a single
 * two-axis offset, exactly what a `UIScrollView` / `NSScrollView` /
 * `overflow: auto` `<div>` report.
 *
 * ## Content contract
 *
 * Holds exactly one child: the inner `FrameLayout` the backend parents
 * every scroll child under. Taffy sizes that inner to the content on
 * both axes and the layout pass writes it as exact `LayoutParams`; this
 * view measures it UNSPECIFIED on both axes — what `ScrollView` does on
 * its one scrolling axis — so the content is never squeezed to the
 * viewport. The scroll range on each axis is the child's extent past
 * the viewport.
 *
 * ## Gesture
 *
 * A drag is claimed once it passes the touch slop on either axis, so a
 * tap still reaches a pressable child. At that moment it locks to the
 * axis it mostly moves along (UIKit's `directionalLockEnabled`, which
 * the iOS backend turns on for the same scroller): a mostly-vertical
 * swipe down a wide table scrolls the rows without drifting the columns.
 * A diagonal drag pans freely. A drag locked to an axis this view
 * cannot scroll is left to an ancestor (a horizontal pager around a
 * table that fits its width still pages). Release flings through an
 * `OverScroller`; overscroll draws the platform `EdgeEffect` on the
 * edge it hit, honouring `overScrollMode` like `ScrollView`.
 */
class RustTwoAxisScrollView(context: Context) :
    FrameLayout(context, null, android.R.attr.scrollViewStyle) {

    private val scroller = OverScroller(context)
    private val touchSlop = ViewConfiguration.get(context).scaledTouchSlop
    private val minFling = ViewConfiguration.get(context).scaledMinimumFlingVelocity
    private val maxFling = ViewConfiguration.get(context).scaledMaximumFlingVelocity

    private val edgeTop = EdgeEffect(context)
    private val edgeBottom = EdgeEffect(context)
    private val edgeLeft = EdgeEffect(context)
    private val edgeRight = EdgeEffect(context)

    private enum class Lock { NONE, X, Y }

    private var downX = 0f
    private var downY = 0f
    private var lastTouchX = 0f
    private var lastTouchY = 0f
    private var dragging = false
    private var lock = Lock.NONE
    private var velocityTracker: VelocityTracker? = null

    init {
        // `scrollViewStyle` installs the theme's vertical scrollbar thumb
        // (and the scroll cache that draws it); the horizontal bar shares
        // the cache. Fading edges stay off on both axes, matching the
        // Material `ScrollView`.
        isVerticalScrollBarEnabled = true
        isHorizontalScrollBarEnabled = true
        isVerticalFadingEdgeEnabled = false
        isHorizontalFadingEdgeEnabled = false
        // `draw` paints the edge effects; a FrameLayout without a
        // background otherwise skips `draw` entirely.
        setWillNotDraw(false)
    }

    // -- measurement ---------------------------------------------------

    override fun measureChild(child: View, parentWidthMeasureSpec: Int, parentHeightMeasureSpec: Int) {
        child.measure(unbounded(parentWidthMeasureSpec), unbounded(parentHeightMeasureSpec))
    }

    override fun measureChildWithMargins(
        child: View,
        parentWidthMeasureSpec: Int,
        widthUsed: Int,
        parentHeightMeasureSpec: Int,
        heightUsed: Int,
    ) {
        child.measure(unbounded(parentWidthMeasureSpec), unbounded(parentHeightMeasureSpec))
    }

    /** UNSPECIFIED with the parent's size as the hint, like
     *  `ScrollView.measureChildWithMargins` on its scrolling axis. */
    private fun unbounded(parentSpec: Int): Int =
        MeasureSpec.makeMeasureSpec(MeasureSpec.getSize(parentSpec), MeasureSpec.UNSPECIFIED)

    override fun onLayout(changed: Boolean, l: Int, t: Int, r: Int, b: Int) {
        super.onLayout(changed, l, t, r, b)
        // Content that shrank (or a viewport that grew) can leave the
        // offset past the new end; pull it back so no blank band shows.
        scrollTo(scrollX, scrollY)
    }

    // -- scroll range --------------------------------------------------

    private fun contentWidth(): Int {
        val child = getChildAt(0) ?: return 0
        val lp = child.layoutParams as? ViewGroup.MarginLayoutParams
        return child.width + (lp?.leftMargin ?: 0) + (lp?.rightMargin ?: 0)
    }

    private fun contentHeight(): Int {
        val child = getChildAt(0) ?: return 0
        val lp = child.layoutParams as? ViewGroup.MarginLayoutParams
        return child.height + (lp?.topMargin ?: 0) + (lp?.bottomMargin ?: 0)
    }

    private fun viewportWidth() = width - paddingLeft - paddingRight
    private fun viewportHeight() = height - paddingTop - paddingBottom
    private fun maxScrollX() = (contentWidth() - viewportWidth()).coerceAtLeast(0)
    private fun maxScrollY() = (contentHeight() - viewportHeight()).coerceAtLeast(0)

    /** Clamped to the content on both axes, so programmatic scrolls
     *  (`ScrollViewHandle::scroll_to`, the robot) and drags share one
     *  bound — the same clamp `ScrollView.scrollTo` applies. */
    override fun scrollTo(x: Int, y: Int) {
        if (childCount == 0) return
        val cx = x.coerceIn(0, maxScrollX())
        val cy = y.coerceIn(0, maxScrollY())
        if (cx != scrollX || cy != scrollY) {
            super.scrollTo(cx, cy)
            awakenScrollBars()
        }
    }

    // Scrollbar geometry + `canScrollHorizontally/Vertically` both read
    // these. View's defaults report the view's own box, i.e. "nothing
    // to scroll".
    override fun computeHorizontalScrollRange(): Int = contentWidth().coerceAtLeast(viewportWidth())
    override fun computeHorizontalScrollExtent(): Int = viewportWidth()
    override fun computeHorizontalScrollOffset(): Int = scrollX
    override fun computeVerticalScrollRange(): Int = contentHeight().coerceAtLeast(viewportHeight())
    override fun computeVerticalScrollExtent(): Int = viewportHeight()
    override fun computeVerticalScrollOffset(): Int = scrollY

    // -- touch ---------------------------------------------------------

    override fun onInterceptTouchEvent(ev: MotionEvent): Boolean {
        when (ev.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                beginTouch(ev)
                // Catching a running fling stops it, and that touch is a
                // scroll touch rather than a tap on whatever slid under it.
                val wasFlinging = !scroller.isFinished
                scroller.forceFinished(true)
                dragging = wasFlinging
                return wasFlinging
            }
            MotionEvent.ACTION_MOVE -> return dragging || maybeStartDrag(ev)
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> dragging = false
        }
        return dragging
    }

    override fun onTouchEvent(ev: MotionEvent): Boolean {
        val tracker = velocityTracker ?: VelocityTracker.obtain().also { velocityTracker = it }
        tracker.addMovement(ev)
        when (ev.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                // No child took the touch (it landed on empty content):
                // keep the stream so a drag from here still scrolls.
                beginTouch(ev)
                scroller.forceFinished(true)
                return true
            }
            MotionEvent.ACTION_MOVE -> {
                if (!dragging && !maybeStartDrag(ev)) return true
                var dx = (lastTouchX - ev.x).roundToInt()
                var dy = (lastTouchY - ev.y).roundToInt()
                lastTouchX = ev.x
                lastTouchY = ev.y
                if (lock == Lock.Y) dx = 0
                if (lock == Lock.X) dy = 0
                pullEdges(scrollX + dx, scrollY + dy, ev)
                scrollTo(scrollX + dx, scrollY + dy)
                return true
            }
            MotionEvent.ACTION_UP -> {
                if (dragging) {
                    tracker.computeCurrentVelocity(1000, maxFling.toFloat())
                    var vx = -tracker.xVelocity.roundToInt()
                    var vy = -tracker.yVelocity.roundToInt()
                    if (lock == Lock.Y || abs(vx) < minFling) vx = 0
                    if (lock == Lock.X || abs(vy) < minFling) vy = 0
                    if (vx != 0 || vy != 0) {
                        scroller.fling(scrollX, scrollY, vx, vy, 0, maxScrollX(), 0, maxScrollY())
                        postInvalidateOnAnimation()
                    }
                }
                endTouch()
                return true
            }
            MotionEvent.ACTION_CANCEL -> {
                endTouch()
                return true
            }
        }
        return super.onTouchEvent(ev)
    }

    private fun beginTouch(ev: MotionEvent) {
        downX = ev.x
        downY = ev.y
        lastTouchX = ev.x
        lastTouchY = ev.y
        dragging = false
        lock = Lock.NONE
    }

    /** Claim the gesture once it passes the slop, choosing the axis
     *  lock from its direction so far. Returns whether it is now ours. */
    private fun maybeStartDrag(ev: MotionEvent): Boolean {
        val adx = abs(ev.x - downX)
        val ady = abs(ev.y - downY)
        if (adx <= touchSlop && ady <= touchSlop) return false
        val wantLock = when {
            adx > ady * DIRECTIONAL_LOCK_RATIO -> Lock.X
            ady > adx * DIRECTIONAL_LOCK_RATIO -> Lock.Y
            else -> Lock.NONE
        }
        val canX = maxScrollX() > 0
        val canY = maxScrollY() > 0
        val ours = when (wantLock) {
            Lock.X -> canX
            Lock.Y -> canY
            Lock.NONE -> canX || canY
        }
        if (!ours) return false
        lock = wantLock
        dragging = true
        // Scroll from where the finger is now, not from the DOWN point:
        // the slop distance is not a scroll the user made.
        lastTouchX = ev.x
        lastTouchY = ev.y
        // Keep an ancestor scroller from also acting on this gesture.
        parent?.requestDisallowInterceptTouchEvent(true)
        return true
    }

    private fun endTouch() {
        velocityTracker?.recycle()
        velocityTracker = null
        dragging = false
        lock = Lock.NONE
        edgeTop.onRelease()
        edgeBottom.onRelease()
        edgeLeft.onRelease()
        edgeRight.onRelease()
        if (!edgesFinished()) postInvalidateOnAnimation()
    }

    // -- fling ---------------------------------------------------------

    override fun computeScroll() {
        if (!scroller.computeScrollOffset()) return
        val oldX = scrollX
        val oldY = scrollY
        val x = scroller.currX
        val y = scroller.currY
        scrollTo(x, y)
        // A fling that runs into an edge hands its momentum to the edge
        // effect, as `ScrollView` does.
        val velocity = scroller.currVelocity.roundToInt()
        if (overscrollOn(maxScrollY())) {
            if (y <= 0 && oldY > 0) edgeTop.onAbsorb(velocity)
            else if (y >= maxScrollY() && oldY < maxScrollY()) edgeBottom.onAbsorb(velocity)
        }
        if (overscrollOn(maxScrollX())) {
            if (x <= 0 && oldX > 0) edgeLeft.onAbsorb(velocity)
            else if (x >= maxScrollX() && oldX < maxScrollX()) edgeRight.onAbsorb(velocity)
        }
        postInvalidateOnAnimation()
    }

    // -- edge effects --------------------------------------------------

    /** `overScrollMode` gate for one axis whose scroll range is `range`. */
    private fun overscrollOn(range: Int): Boolean = when (overScrollMode) {
        View.OVER_SCROLL_ALWAYS -> true
        View.OVER_SCROLL_IF_CONTENT_SCROLLS -> range > 0
        else -> false
    }

    /** Feed the part of a drag that runs past an edge to that edge. */
    private fun pullEdges(targetX: Int, targetY: Int, ev: MotionEvent) {
        val vw = viewportWidth().coerceAtLeast(1)
        val vh = viewportHeight().coerceAtLeast(1)
        if (overscrollOn(maxScrollY())) {
            if (targetY < 0) {
                edgeTop.onPull(-targetY.toFloat() / vh, ev.x / vw)
                postInvalidateOnAnimation()
            } else if (targetY > maxScrollY()) {
                edgeBottom.onPull((targetY - maxScrollY()).toFloat() / vh, 1f - ev.x / vw)
                postInvalidateOnAnimation()
            }
        }
        if (overscrollOn(maxScrollX())) {
            if (targetX < 0) {
                edgeLeft.onPull(-targetX.toFloat() / vw, 1f - ev.y / vh)
                postInvalidateOnAnimation()
            } else if (targetX > maxScrollX()) {
                edgeRight.onPull((targetX - maxScrollX()).toFloat() / vw, ev.y / vh)
                postInvalidateOnAnimation()
            }
        }
    }

    private fun edgesFinished() =
        edgeTop.isFinished && edgeBottom.isFinished && edgeLeft.isFinished && edgeRight.isFinished

    override fun draw(canvas: Canvas) {
        super.draw(canvas)
        if (edgesFinished()) return
        // The canvas is in content space: the viewport's top-left is at
        // (scrollX, scrollY). Each effect draws along its own x axis with
        // the glow growing down its y axis, so it is rotated onto its edge.
        val sx = scrollX.toFloat()
        val sy = scrollY.toFloat()
        val w = width
        val h = height
        var more = false
        if (!edgeTop.isFinished) {
            val save = canvas.save()
            canvas.translate(sx, sy)
            edgeTop.setSize(w, h)
            more = edgeTop.draw(canvas) or more
            canvas.restoreToCount(save)
        }
        if (!edgeBottom.isFinished) {
            val save = canvas.save()
            canvas.translate(sx - w, sy + h)
            canvas.rotate(180f, w.toFloat(), 0f)
            edgeBottom.setSize(w, h)
            more = edgeBottom.draw(canvas) or more
            canvas.restoreToCount(save)
        }
        if (!edgeLeft.isFinished) {
            val save = canvas.save()
            canvas.rotate(270f)
            canvas.translate(-(sy + h), sx)
            edgeLeft.setSize(h, w)
            more = edgeLeft.draw(canvas) or more
            canvas.restoreToCount(save)
        }
        if (!edgeRight.isFinished) {
            val save = canvas.save()
            canvas.rotate(90f)
            canvas.translate(sy, -(sx + w))
            edgeRight.setSize(h, w)
            more = edgeRight.draw(canvas) or more
            canvas.restoreToCount(save)
        }
        if (more) postInvalidateOnAnimation()
    }

    private companion object {
        /** A drag whose movement along one axis exceeds this multiple of
         *  the other locks to that axis — within ~26.6° (atan 1/2) of
         *  straight. Steeper diagonals pan both axes. */
        const val DIRECTIONAL_LOCK_RATIO = 2f
    }
}
