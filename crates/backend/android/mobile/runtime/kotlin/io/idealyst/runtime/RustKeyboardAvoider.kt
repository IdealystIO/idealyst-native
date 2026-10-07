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
 *   move ([nativeBeginPadding]) and hands back the views that move, each
 *   with its old and new top ([setTargets]). Each frame, every moved view's
 *   VISUAL top goes `old → new` by the animation's progress, written as
 *   `translationY = desired − getTop()`. Measuring against the view's
 *   actual top is what makes it robust: a layout pass writes
 *   `LayoutParams`, which only take effect at the next traversal, so on any
 *   given frame the view may still be at its old position or already at
 *   its new one (assuming either produced a 2-frame jump on close). A
 *   layout listener re-places a target whenever its layout lands. Sizes
 *   switch where the keyboard hides it: mode 0 (content grows) is laid out
 *   at the start; mode 1 (content shrinks) via [nativeCommitPadding] at the
 *   end. See `soft_keyboard_policy.rs`.
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
    /** Padding: views moving in the running animation, their old / new
     *  tops (px, as `getTop()` reports them), their translation before it
     *  started, the animation's progress and the mode (see class doc). */
    private var targets: Array<View> = emptyArray()
    private var oldTops = IntArray(0)
    private var newTops = IntArray(0)
    private var bases = FloatArray(0)
    private var progress = 0f
    private var mode = 0
    private var active = false

    /** Re-places a target when its layout lands (see class doc); detaches
     *  itself once the target is at its new top with the move finished. */
    private val relayout = View.OnLayoutChangeListener { v, _, _, _, _, _, _, _, _ ->
        val i = targets.indexOf(v)
        if (i >= 0) place(i)
    }

    private fun place(i: Int) {
        val v = targets[i]
        val desired = oldTops[i] + (newTops[i] - oldTops[i]) * progress
        v.translationY = bases[i] + desired - v.top
        if (!active && v.top == newTops[i]) {
            v.translationY = bases[i]
            v.removeOnLayoutChangeListener(relayout)
        }
    }
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
        progress = fraction
        for (i in targets.indices) place(i)
    }

    fun onEnd(finalImePx: Int) {
        if (detached) return
        if (!animated) return
        if (behavior == TRANSLATE) {
            view.translationY = -overlapDp(finalImePx) * density
            return
        }
        // Hold every moved view at its NEW top (by translation, until its
        // layout gets there), then apply the final layout. The relayout
        // listener drops each offset in the frame that view's layout lands,
        // so nothing jumps.
        progress = 1f
        active = false
        val finalDp = overlapDp(finalImePx)
        val committed = finalDp == appliedDp || nativeCommitPadding(key, finalDp)
        for (i in targets.indices) place(i)
        if (!committed) {
            view.post { applyStatic(finalImePx) }
            return
        }
        appliedDp = finalDp
    }

    /** From Rust ([nativeBeginPadding]): the views this move translates. */
    fun setTargets(views: Array<View>, oldTopsPx: IntArray, newTopsPx: IntArray, mode: Int) {
        // A view's translation is the author's / animation's plus ours; keep
        // theirs as the base. Between moves our offset is 0, so that is its
        // current translation — except for a view still settling from an
        // interrupted move, which keeps the base it had.
        val previous = targets.indices.associate { targets[it] to bases[it] }
        for (v in targets) v.removeOnLayoutChangeListener(relayout)
        targets = views
        oldTops = oldTopsPx
        newTops = newTopsPx
        bases = FloatArray(views.size) { i -> previous[views[i]] ?: views[i].translationY }
        progress = 0f
        this.mode = mode
        if (mode == GROW_NOW) appliedDp = targetDp
        for (i in views.indices) {
            views[i].addOnLayoutChangeListener(relayout)
            place(i)
        }
    }

    /** The view was released (from Rust). */
    fun detach() {
        detached = true
        for (i in targets.indices) {
            targets[i].removeOnLayoutChangeListener(relayout)
            targets[i].translationY = bases[i]
        }
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
