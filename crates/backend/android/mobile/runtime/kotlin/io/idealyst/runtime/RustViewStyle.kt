package io.idealyst.runtime

import android.view.View

/**
 * View setters every style pass writes, in one JNI call.
 */
object RustViewStyle {
    /**
     * The scroll-surface properties (`overscroll_behavior`, `scrollbar`).
     * Written on every pass, on every view, so a reactive flip back to
     * `Auto` restores the default; a view that does not scroll ignores
     * them. Three setters from Rust were three by-name JNI calls per
     * styled view.
     */
    @JvmStatic
    fun scrollSurface(view: View, overScrollMode: Int, showScrollBars: Boolean) {
        view.overScrollMode = overScrollMode
        view.isHorizontalScrollBarEnabled = showScrollBars
        view.isVerticalScrollBarEnabled = showScrollBars
    }
}
