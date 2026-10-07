package io.idealyst.runtime

import android.view.View

/**
 * One `keyboard_avoiding_view`. Created by the Rust backend
 * (`imp::soft_keyboard::mark`) and driven by [RustKeyboardInsets], which
 * forwards every phase of the system IME animation — so the motion comes
 * from the platform's own keyboard animation, frame by frame.
 *
 * The per-frame work is entirely here, as `translationY` writes (a GPU
 * property: no layout, no redraw of content, no call into Rust):
 *
 * - **Translate** (`behavior = 1`): the view's own `translationY` follows
 *   the live IME inset.
 * - **Padding** (`behavior = 0`): Rust lays out ONCE at the start of the
 *   move ([nativeBeginPadding]) and hands back the views that moved and by
 *   how much ([setTargets]); each frame offsets them by the animation's
 *   progress. Sizes switch where the keyboard hides it: mode 0 (content
 *   grows) laid out at the start, offsets run `-dy → 0`; mode 1 (content
 *   shrinks) offsets run `0 → dy`, then [nativeCommitPadding] lays out the
 *   smaller size at the end. See `soft_keyboard_policy.rs`.
 *
 * Overlap is measured against the view's UNTRANSLATED bottom edge, so a
 * Translate lift never feeds back into its own measurement.
 */
class RustKeyboardAvoider private constructor(
    private val view: View,
    private val key: Long,
    private val behavior: Int,
    private val animated: Boolean,
) {
    /** Keyboard padding (dp) currently laid out, for Padding. */
    private var appliedDp = 0f
    /** Where the running move is heading (dp). */
    private var targetDp = 0f
    /** Padding: views moving in the running animation, their offsets (px),
     *  their translation before it started, and the mode (see class doc). */
    private var targets: Array<View> = emptyArray()
    private var offsets = FloatArray(0)
    private var bases = FloatArray(0)
    private var mode = 0
    private var active = false
    private var detached = false

    private val density: Float
        get() = view.resources.displayMetrics.density.let { if (it > 0f) it else 1f }

    /** How much of this view (dp) an IME inset of [imePx] covers. */
    private fun overlapDp(imePx: Int): Float {
        val loc = IntArray(2)
        view.getLocationInWindow(loc)
        val bottom = loc[1] - view.translationY + view.height
        val windowHeight = view.rootView.height
        val keyboardTop = windowHeight - imePx
        return maxOf(0f, bottom - keyboardTop) / density
    }

    /** A change the system did not animate (or `animated = false`). */
    fun applyStatic(imePx: Int) {
        if (detached) return
        val dp = overlapDp(imePx)
        if (behavior == TRANSLATE) {
            view.translationY = -dp * density
        } else if (dp != appliedDp) {
            if (nativeCommitPadding(key, dp)) {
                appliedDp = dp
            } else {
                // Backend busy (a flush is running): retry next frame.
                view.post { applyStatic(imePx) }
            }
        }
    }

    fun onStart(endImePx: Int) {
        if (detached) return
        if (!animated) {
            applyStatic(endImePx)
            return
        }
        targetDp = overlapDp(endImePx)
        if (behavior == PADDING && targetDp != appliedDp) {
            active = nativeBeginPadding(key, appliedDp, targetDp)
        }
    }

    fun onProgress(imePx: Int, fraction: Float) {
        if (detached || !animated) return
        if (behavior == TRANSLATE) {
            view.translationY = -overlapDp(imePx) * density
            return
        }
        if (!active) return
        for (i in targets.indices) {
            val offset = if (mode == GROW_NOW) -offsets[i] * (1f - fraction) else offsets[i] * fraction
            targets[i].translationY = bases[i] + offset
        }
    }

    fun onEnd(finalImePx: Int) {
        if (detached) return
        if (!animated) return
        if (behavior == TRANSLATE) {
            view.translationY = -overlapDp(finalImePx) * density
            return
        }
        // Same frame: drop the offsets and apply the final layout, so views
        // land exactly where the offsets were taking them.
        for (i in targets.indices) targets[i].translationY = bases[i]
        targets = emptyArray()
        active = false
        val finalDp = overlapDp(finalImePx)
        if (finalDp != appliedDp && !nativeCommitPadding(key, finalDp)) {
            view.post { applyStatic(finalImePx) }
            return
        }
        appliedDp = finalDp
    }

    /** From Rust ([nativeBeginPadding]): the views this move translates. */
    fun setTargets(views: Array<View>, offsetsPx: FloatArray, mode: Int) {
        targets = views
        offsets = offsetsPx
        bases = FloatArray(views.size) { views[it].translationY }
        this.mode = mode
        if (mode == GROW_NOW) {
            // The larger layout is already applied: start every moved view
            // at its old position so nothing jumps.
            for (i in views.indices) views[i].translationY = bases[i] - offsetsPx[i]
            appliedDp = targetDp
        }
    }

    /** The view was released (from Rust). */
    fun detach() {
        detached = true
        for (i in targets.indices) targets[i].translationY = bases[i]
        targets = emptyArray()
        RustKeyboardInsets.avoiders.remove(this)
    }

    private external fun nativeBeginPadding(key: Long, fromDp: Float, toDp: Float): Boolean
    private external fun nativeCommitPadding(key: Long, dp: Float): Boolean

    companion object {
        private const val PADDING = 0
        private const val TRANSLATE = 1
        private const val GROW_NOW = 0

        /** Called from Rust during the view's mount. A keyboard that is
         *  already up is applied on the next main-thread turn: the backend
         *  is borrowed for this mount, and Padding calls back into it. */
        @JvmStatic
        fun attach(view: View, key: Long, behavior: Int, animated: Boolean): RustKeyboardAvoider {
            val a = RustKeyboardAvoider(view, key, behavior, animated)
            RustKeyboardInsets.avoiders.add(a)
            view.post {
                val ime = RustKeyboardInsets.currentImePx(view)
                if (ime > 0) a.applyStatic(ime)
            }
            return a
        }
    }
}
