package io.idealyst.runtime

import android.app.Activity
import android.app.ActivityManager
import android.content.Context
import android.graphics.Rect
import android.os.Build
import android.util.Log
import android.view.View
import android.view.WindowManager
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

/**
 * System-UI app controls. Called from Rust (`backend-android`'s
 * `Backend::fullscreen_setter`) via JNI as a single static method, so
 * the per-API-level `WindowInsetsController` dance lives here in Kotlin
 * (via `androidx.core`'s compat shims) instead of being reconstructed
 * across JNI.
 *
 * Static, additive surface — there's no critical path through here, so
 * an embedded runtime older than the app's native lib simply lacks the
 * class/method and the Rust caller soft-fails instead of aborting on a
 * missing JNI method.
 */
object RustSystemUi {
    /** Layout listener kept so we can re-assert the gesture-exclusion
     *  rects whenever the decor view resizes (rotation, multi-window),
     *  and remove them again on exit. Single shared field — there's one
     *  host Activity. */
    private var exclusionListener: View.OnLayoutChangeListener? = null

    /** The author's `set_fullscreen` request. */
    private var fullscreenRequested = false

    /** The task is screen-pinned or in lock-task (kiosk) mode. */
    private var pinned = false

    /**
     * Enter (`enabled = true`) or leave immersive full-screen.
     *
     * Two coupled effects, because on Android they only work together
     * (see Chris Banes' gesture-nav series):
     *
     * 1. **Immersive-sticky** — hide the status + navigation bars with
     *    `BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE`. This is the only state
     *    in which the system *lifts* the 200dp-per-edge cap on gesture
     *    exclusion.
     * 2. **Full-bounds system-gesture exclusion** on the decor view —
     *    with the cap lifted, this hands every left/right edge swipe to
     *    the app as a normal touch instead of the system back gesture.
     *    So an edge swipe on a drawing canvas becomes a stroke: no back
     *    arrow, no navigation, and (crucially) no transient-bar flash,
     *    because the swipe never reaches the "reveal bars" path. The
     *    bottom home gesture stays mandatory and is unaffected.
     *
     * Leaving restores the default gesture areas, and the bars unless the
     * task is pinned ([watchPinning]).
     */
    @JvmStatic
    fun setFullscreen(context: Context, enabled: Boolean) {
        val activity = context as? Activity ?: run {
            Log.w(
                "idealyst",
                "RustSystemUi.setFullscreen: context is not an Activity " +
                    "(${context.javaClass.name}); ignoring",
            )
            return
        }
        // WindowInsetsController / exclusion rects must be touched on the
        // UI thread. The framework calls this from a main-thread event
        // handler, so runOnUiThread runs inline; it's a guard for any
        // off-thread caller.
        activity.runOnUiThread {
            fullscreenRequested = enabled
            val decor = activity.window.decorView
            if (enabled) applyEdgeExclusion(decor) else clearEdgeExclusion(decor)
            applyBars(activity)
        }
    }

    /**
     * Hide the system bars while the task is pinned (screen pinning or
     * lock-task mode), so the app fills the screen.
     *
     * Pinning empties the status bar and the taskbar but leaves both
     * reporting their full insets as visible, and dispatches no insets
     * change — so the host root stays inset by margins around a blank
     * 24dp strip and a black 60dp taskbar slab. Hiding the bars is what
     * makes the system report zero insets (and dispatch them).
     *
     * There is no pinning callback. Pinning lays the window out, so the
     * lock-task state is re-read on every global layout. Unpinning does
     * not (the hidden bars stay hidden, nothing moves), so while pinned
     * the state is also polled every [PIN_POLL_MS].
     */
    @JvmStatic
    fun watchPinning(activity: Activity, root: View) {
        val am = activity.getSystemService(Context.ACTIVITY_SERVICE) as? ActivityManager ?: return
        lateinit var poll: Runnable
        val sync = {
            val now = am.lockTaskModeState != ActivityManager.LOCK_TASK_MODE_NONE
            if (now != pinned) {
                pinned = now
                applyBars(activity)
                // One poll chain at a time: a quick unpin + re-pin can
                // land while the previous chain's last check is pending.
                root.removeCallbacks(poll)
                if (now) root.postDelayed(poll, PIN_POLL_MS)
            }
        }
        poll = Runnable {
            sync()
            if (pinned) {
                root.removeCallbacks(poll)
                root.postDelayed(poll, PIN_POLL_MS)
            }
        }
        root.viewTreeObserver.addOnGlobalLayoutListener { sync() }
        sync()
    }

    /** How often a pinned app checks whether it has been unpinned. */
    private const val PIN_POLL_MS = 500L

    /** Show or hide the bars for `fullscreenRequested || pinned`. */
    private fun applyBars(activity: Activity) {
        val window = activity.window
        val controller = WindowInsetsControllerCompat(window, window.decorView)
        if (fullscreenRequested || pinned) {
            WindowCompat.setDecorFitsSystemWindows(window, false)
            // Draw into the display cutout so the window background fills
            // behind it (API 28+); without this the cutout strip letterboxes
            // black while the bars are hidden — leaving a black band at the
            // top even though the app paints the rest of the screen.
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                window.attributes = window.attributes.apply {
                    layoutInDisplayCutoutMode =
                        WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
                }
            }
            controller.hide(WindowInsetsCompat.Type.systemBars())
            controller.systemBarsBehavior =
                WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        } else {
            controller.show(WindowInsetsCompat.Type.systemBars())
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                window.attributes = window.attributes.apply {
                    layoutInDisplayCutoutMode =
                        WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_DEFAULT
                }
            }
            // Stay edge to edge when the keyboard tracker owns the
            // insets: it keeps the root inside the bars itself, and
            // fitting windows would hand the IME back to the system's
            // one-step window resize (see RustKeyboardInsets).
            WindowCompat.setDecorFitsSystemWindows(window, !RustKeyboardInsets.installed)
        }
    }

    /** Exclude the decor view's full bounds from the system back
     *  gesture, re-asserted on every layout. `setSystemGestureExclusionRects`
     *  is API 29+; older devices have no gesture-nav back to exclude, so
     *  this is a no-op there (the immersive bars still apply). */
    private fun applyEdgeExclusion(decor: View) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
        clearEdgeExclusion(decor)
        val apply = {
            // Full bounds: only the left/right edge regions inside it are
            // gesture areas, so this reclaims back on both edges. The
            // 200dp cap that would normally clamp this is lifted while the
            // system bars are hidden (immersive).
            decor.systemGestureExclusionRects =
                listOf(Rect(0, 0, decor.width, decor.height))
        }
        apply()
        val listener = View.OnLayoutChangeListener {
            _, _, _, _, _, _, _, _, _ ->
            apply()
        }
        decor.addOnLayoutChangeListener(listener)
        exclusionListener = listener
    }

    /** Drop the exclusion rects + the layout listener. */
    private fun clearEdgeExclusion(decor: View) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
        exclusionListener?.let { decor.removeOnLayoutChangeListener(it) }
        exclusionListener = null
        decor.systemGestureExclusionRects = emptyList()
    }
}
