package io.idealyst.runtime

import android.content.Context
import android.util.Log
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import androidx.drawerlayout.widget.DrawerLayout

/**
 * `FrameLayout` subclass that always measures itself and its
 * single child with `MeasureSpec.EXACTLY`, regardless of what the
 * parent passed in.
 *
 * Why: `DrawerLayout.onMeasure` requires its parent to measure
 * it with `EXACTLY` (it throws `IllegalArgumentException`
 * otherwise). When the drawer sits inside a parent measured
 * `AT_MOST` (e.g. `RustNavigator`'s container, which is sized
 * `WRAP_CONTENT` so it can host scrollable screens), the
 * `AT_MOST` spec propagates down and trips the assertion.
 *
 * This wrapper sits between the parent FrameLayout and the
 * DrawerLayout: it accepts whatever spec the parent gives, but
 * resolves its own size to the parent's reported maximum and
 * measures its child with `EXACTLY` of that size. The drawer is
 * happy; the parent gets the same final size it would have
 * gotten anyway.
 */
class RustExactFrameLayout(context: Context) : FrameLayout(context) {
    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        // Pick the size each axis. For AT_MOST / EXACTLY, take
        // the bound the parent gave us; for UNSPECIFIED fall
        // back to the suggested minimum (rare in practice — only
        // top-level Activity views see UNSPECIFIED).
        val width = resolveAxis(widthMeasureSpec, suggestedMinimumWidth)
        val height = resolveAxis(heightMeasureSpec, suggestedMinimumHeight)

        val exactWidth = MeasureSpec.makeMeasureSpec(width, MeasureSpec.EXACTLY)
        val exactHeight = MeasureSpec.makeMeasureSpec(height, MeasureSpec.EXACTLY)

        // Measure each child with EXACTLY of the resolved size.
        // The framework's drawer is always exactly one child
        // (the DrawerLayout); other configurations work too.
        for (i in 0 until childCount) {
            getChildAt(i)?.measure(exactWidth, exactHeight)
        }

        setMeasuredDimension(width, height)
    }

    private fun resolveAxis(spec: Int, fallback: Int): Int {
        val size = MeasureSpec.getSize(spec)
        return when (MeasureSpec.getMode(spec)) {
            MeasureSpec.EXACTLY, MeasureSpec.AT_MOST -> size
            else -> fallback // UNSPECIFIED
        }
    }
}

/**
 * `androidx.drawerlayout.widget.DrawerLayout` wrapper for the
 * framework's `DrawerNavigator`.
 *
 * # Children
 *
 * DrawerLayout requires exactly two children:
 *
 *   1. **content view** — fills the whole frame (the active
 *      screen). Layout params: MATCH_PARENT × MATCH_PARENT,
 *      no gravity.
 *   2. **drawer view** — slides in from the start edge. Layout
 *      params: WRAP_CONTENT × MATCH_PARENT, gravity = START (or END
 *      for `DrawerSide::End`, set via [setDrawerGravity]).
 *
 * The Rust backend builds both views, then calls [attachContent]
 * and [attachDrawer] in that order so DrawerLayout sees the
 * expected children list when it lays out.
 *
 * # Lifecycle of `nativePtr`
 *
 * The pointer is a leaked `Box<DrawerListenerBox>` allocated in
 * Rust by `create_drawer`. We invoke it via [nativeOnDrawerOpened]
 * / [nativeOnDrawerClosed] on every drawer state transition; Rust
 * uses the pointer to look up the per-navigator state and flip
 * the `is_open` signal. Freed in Rust's `release_drawer_navigator`.
 *
 * # Why a custom subclass and not raw DrawerLayout?
 *
 * The framework needs a `RustDrawerLayout` class file we can
 * `findClass` from JNI, and a stable JNI method surface for
 * dispatch. Subclassing also lets us cache the drawer-child reference
 * so open/close calls don't have to walk the child list every time.
 */
class RustDrawerLayout(
    context: Context,
    private val nativePtr: Long,
) : DrawerLayout(context) {

    /**
     * The drawer (side panel) child. `null` until [attachDrawer]
     * runs. open/close/toggle methods look it up via this field
     * rather than walking children — DrawerLayout's
     * `openDrawer(gravity)` overload exists but takes a gravity
     * int rather than a View, and we already have the View handle.
     */
    private var drawerChild: View? = null

    private var drawerGravity: Int = Gravity.START

    init {
        // DrawerLayout fires this listener on every drawer state
        // change. We bridge the open/close events into Rust so the
        // framework's reactive `is_open` signal stays in sync with
        // the native widget. Slide progress + state-changed events
        // are unused for now (we don't expose them to authors yet).
        addDrawerListener(object : DrawerListener {
            override fun onDrawerSlide(drawerView: View, slideOffset: Float) {}
            override fun onDrawerOpened(drawerView: View) {
                nativeOnDrawerOpened(nativePtr)
            }
            override fun onDrawerClosed(drawerView: View) {
                nativeOnDrawerClosed(nativePtr)
            }
            override fun onDrawerStateChanged(newState: Int) {}
        })
    }

    /**
     * Set the side the drawer opens from. Must be called BEFORE
     * [attachDrawer] — DrawerLayout reads gravity off the child's
     * `LayoutParams`.
     */
    fun setDrawerGravity(gravity: Int) {
        drawerGravity = gravity
    }

    /**
     * Insert the content view (the active screen container). Sized
     * to fill the drawer-layout. Must be added BEFORE [attachDrawer]
     * so the z-order is correct (drawer renders above content).
     */
    fun attachContent(content: View) {
        val lp = LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT,
        )
        addView(content, lp)
    }

    /**
     * Insert the drawer (side panel) view. Sized to its intrinsic
     * width with full height. Gravity = [drawerGravity] determines
     * which edge it slides from.
     */
    fun attachDrawer(drawer: View) {
        Log.i("idealyst", "attachDrawer: before addView childCount=" + childCount
            + " drawer=" + drawer + " drawerParent=" + drawer.parent)
        val lp = LayoutParams(
            ViewGroup.LayoutParams.WRAP_CONTENT,
            ViewGroup.LayoutParams.MATCH_PARENT,
        )
        lp.gravity = drawerGravity
        addView(drawer, lp)
        drawerChild = drawer
        Log.i("idealyst", "attachDrawer: after addView childCount=" + childCount
            + " drawerInTree=" + (drawer.parent === this))
    }

    /**
     * Open the drawer (animated). No-op if no drawer is attached
     * or it's already open.
     */
    fun openDrawerProgrammatic() {
        val d = drawerChild ?: return
        if (!isDrawerOpen(d)) {
            openDrawer(d)
        }
    }

    /**
     * Close the drawer (animated). No-op if not attached or
     * already closed.
     */
    fun closeDrawerProgrammatic() {
        val d = drawerChild ?: return
        if (isDrawerOpen(d)) {
            closeDrawer(d)
        }
    }

    /**
     * Toggle the drawer. No-op if not attached.
     */
    fun toggleDrawer() {
        val d = drawerChild ?: return
        if (isDrawerOpen(d)) {
            closeDrawer(d)
        } else {
            openDrawer(d)
        }
    }

    /**
     * Enable/disable edge-swipe-to-open. When disabled, the drawer
     * can still be opened programmatically via
     * [openDrawerProgrammatic].
     *
     * `LOCK_MODE_UNLOCKED` = swipe enabled (default).
     * `LOCK_MODE_LOCKED_CLOSED` = no swipe; drawer can only be
     *   opened programmatically.
     */
    fun setSwipeEnabled(enabled: Boolean) {
        val d = drawerChild
        val mode = if (enabled) LOCK_MODE_UNLOCKED else LOCK_MODE_LOCKED_CLOSED
        if (d != null) {
            setDrawerLockMode(mode, d)
        } else {
            // No drawer attached yet — apply globally so the lock
            // mode is in effect by the time the drawer attaches.
            setDrawerLockMode(mode)
        }
    }

    private external fun nativeOnDrawerOpened(ptr: Long)
    private external fun nativeOnDrawerClosed(ptr: Long)

    @Suppress("unused")
    private external fun nativeDrop(ptr: Long)

    companion object {
        private const val TAG = "RustDrawerLayout"
    }
}
