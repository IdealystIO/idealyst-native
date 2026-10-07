package io.idealyst.runtime

import android.app.Activity
import android.content.Context
import android.util.Log
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import androidx.core.view.OnApplyWindowInsetsListener
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsAnimationCompat
import androidx.core.view.WindowInsetsCompat

/**
 * Owns the soft keyboard (IME) for the framework, on the Activity's host
 * root (installed by `imp::soft_keyboard::install`). The app viewport never
 * shrinks for the keyboard; only `keyboard_avoiding_view`s
 * ([RustKeyboardAvoider]) avoid it. Pattern from Google's
 * WindowInsetsAnimation sample (`RootViewDeferringInsetsCallback`):
 *
 * 1. **System bars as root margins.** The window is laid out edge to edge
 *    (`setDecorFitsSystemWindows(false)`) — the only mode in which the
 *    system hands IME movement to the app (as per-frame animation
 *    callbacks) instead of resizing the window in one step. To keep every
 *    app's layout exactly as before (content between the status and
 *    navigation bars), the system-bar insets are re-applied as margins on
 *    the host root, which is what `decorFitsSystemWindows(true)` did.
 *    Insets are returned CONSUMED, as the fitting decor used to consume them.
 * 2. **IME animation, forwarded.** Every phase of the system keyboard
 *    animation goes to each registered [RustKeyboardAvoider] (the frames
 *    come from the system animation itself), and where the keyboard is
 *    heading goes to Rust (`nativeKeyboardTarget`) for author code's
 *    `keyboard_inset()`. While an IME animation runs, `onApplyWindowInsets`
 *    (which receives the END state up front) is not forwarded, or avoiders
 *    would jump to the end before the keyboard moves.
 *
 * `WindowInsetsAnimationCompat` backports the callbacks to API 21+ (below 30
 * it animates with its own estimate of the IME curve, which needs
 * `SOFT_INPUT_ADJUST_RESIZE`).
 */
class RustKeyboardInsets private constructor(private val root: View) :
    WindowInsetsAnimationCompat.Callback(DISPATCH_MODE_STOP),
    OnApplyWindowInsetsListener {

    /** True from `onPrepare` to `onEnd` of an IME animation. */
    private var imeAnimating = false

    override fun onApplyWindowInsets(v: View, insets: WindowInsetsCompat): WindowInsetsCompat {
        applyBarMargins(insets)
        if (!imeAnimating) {
            // A non-animated change (rotation, IME switched, hardware
            // keyboard attached, first dispatch): apply it immediately.
            nativeKeyboardTarget(hostOverlapDp(insets), 0L)
            val ime = imePx(insets)
            avoiders.toList().forEach { it.applyStatic(ime) }
        }
        return WindowInsetsCompat.CONSUMED
    }

    override fun onPrepare(animation: WindowInsetsAnimationCompat) {
        if (isIme(animation)) imeAnimating = true
    }

    override fun onStart(
        animation: WindowInsetsAnimationCompat,
        bounds: WindowInsetsAnimationCompat.BoundsCompat,
    ): WindowInsetsAnimationCompat.BoundsCompat {
        if (isIme(animation)) {
            // By `onStart` the root insets already hold the END state.
            ViewCompat.getRootWindowInsets(root)?.let { end ->
                nativeKeyboardTarget(hostOverlapDp(end), animation.durationMillis)
                val ime = imePx(end)
                avoiders.toList().forEach { it.onStart(ime) }
            }
        }
        return bounds
    }

    override fun onProgress(
        insets: WindowInsetsCompat,
        runningAnimations: MutableList<WindowInsetsAnimationCompat>,
    ): WindowInsetsCompat {
        val anim = runningAnimations.firstOrNull { isIme(it) } ?: return insets
        val ime = imePx(insets)
        avoiders.toList().forEach { it.onProgress(ime, anim.interpolatedFraction) }
        return insets
    }

    override fun onEnd(animation: WindowInsetsAnimationCompat) {
        if (!isIme(animation)) return
        imeAnimating = false
        // Settle on the real end state (a cancelled animation stops short).
        ViewCompat.getRootWindowInsets(root)?.let { end ->
            nativeKeyboardTarget(hostOverlapDp(end), 0L)
            val ime = imePx(end)
            avoiders.toList().forEach { it.onEnd(ime) }
        }
    }

    private fun isIme(animation: WindowInsetsAnimationCompat): Boolean =
        (animation.typeMask and WindowInsetsCompat.Type.ime()) != 0

    /** The IME inset (px, measured from the window's bottom). */
    private fun imePx(insets: WindowInsetsCompat): Int =
        insets.getInsets(WindowInsetsCompat.Type.ime()).bottom

    /** The keyboard's overlap of the host root (dp) — `keyboard_inset()`. */
    private fun hostOverlapDp(insets: WindowInsetsCompat): Float {
        val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars()).bottom
        val density = root.resources.displayMetrics.density
        return maxOf(0, imePx(insets) - bars) / (if (density > 0f) density else 1f)
    }

    /** Keep the root inside the system bars, as the fitting decor did. */
    private fun applyBarMargins(insets: WindowInsetsCompat) {
        val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
        val lp = root.layoutParams as? ViewGroup.MarginLayoutParams ?: return
        if (lp.leftMargin == bars.left && lp.topMargin == bars.top &&
            lp.rightMargin == bars.right && lp.bottomMargin == bars.bottom
        ) {
            return
        }
        lp.setMargins(bars.left, bars.top, bars.right, bars.bottom)
        root.layoutParams = lp
    }

    private external fun nativeKeyboardTarget(heightDp: Float, durationMs: Long)

    companion object {
        /** Live `keyboard_avoiding_view`s ([RustKeyboardAvoider.attach] /
         *  [RustKeyboardAvoider.detach]). Main thread only. */
        @JvmStatic
        val avoiders: MutableList<RustKeyboardAvoider> = mutableListOf()

        /** The current IME inset (px from the window bottom), for an avoider
         *  attached while the keyboard is already up. */
        @JvmStatic
        fun currentImePx(view: View): Int =
            ViewCompat.getRootWindowInsets(view)?.getInsets(WindowInsetsCompat.Type.ime())?.bottom ?: 0

        /**
         * True once installed on this process's Activity. `RustSystemUi`
         * reads it so leaving full-screen keeps the window edge to edge
         * (re-enabling `decorFitsSystemWindows` would hand the IME back to
         * the system's one-step window resize).
         */
        @JvmStatic
        var installed: Boolean = false
            private set

        /** Install on `root` (the framework's host root). Returns false when
         *  `context` is not an Activity (nothing to make edge to edge). */
        @JvmStatic
        fun install(context: Context, root: View): Boolean {
            val activity = context as? Activity ?: run {
                Log.w("idealyst", "RustKeyboardInsets.install: context is not an Activity; skipping")
                return false
            }
            val window = activity.window
            WindowCompat.setDecorFitsSystemWindows(window, false)
            window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
            val cb = RustKeyboardInsets(root)
            ViewCompat.setWindowInsetsAnimationCallback(root, cb)
            ViewCompat.setOnApplyWindowInsetsListener(root, cb)
            ViewCompat.requestApplyInsets(root)
            installed = true
            return true
        }
    }
}
