package io.idealyst.runtime

import android.view.View
import android.view.ViewGroup
import androidx.drawerlayout.widget.DrawerLayout

/**
 * JVM-side batch applier for Taffy-computed layout frames.
 *
 * The Rust layout pass computes a frame (left/top/width/height in
 * device px) for every registered view. Applying a frame from Rust used
 * to be ~9 JNI crossings per view — `getLayoutParams`, an `instanceof`
 * check, six `MarginLayoutParams` field writes, and `setLayoutParams`.
 * For a 200-view tree that is ~1800 boundary crossings per layout pass,
 * the bulk of the pass's wall-clock (iOS gets this for free via cached
 * objc selectors / direct dispatch).
 *
 * [applyFrames] collapses that to a SINGLE Rust→JVM call. Rust hands
 * over a parallel `View[]` and an `int[]` of `[left, top, width,
 * height]` quads (index `i` of the view array pairs with quad `i*4`),
 * and this loop performs every `MarginLayoutParams` write JVM-side,
 * where each access is a cheap virtual call rather than a JNI
 * transition. Only the one call plus the two array marshals cross the
 * boundary, regardless of view count.
 *
 * The per-view logic mirrors the old `apply_frame_to_layout_params`
 * exactly:
 *  - A drawer-panel child (a direct child of a `DrawerLayout`) is
 *    skipped: the DrawerLayout owns its child's size/position via its
 *    own `DrawerLayout.LayoutParams` (gravity-based for the panel,
 *    full-bleed for content), and overwriting it would expand the panel
 *    to the full width and defeat the configured `drawer_width`.
 *  - A view whose current LayoutParams isn't a `MarginLayoutParams`
 *    (or subclass, e.g. `FrameLayout.LayoutParams`) gets a fresh one so
 *    margins can be written.
 *  - Trailing margins are zeroed so stale values from a prior pass
 *    don't leak through.
 *
 * Rust pre-filters zero-size views and detached window roots, so every
 * entry handed here is meant to be applied.
 */
object RustLayoutApply {
    @JvmStatic
    fun applyFrames(views: Array<View?>, frames: IntArray) {
        val n = views.size
        var i = 0
        while (i < n) {
            val v = views[i]
            val base = i * 4
            i++
            if (v == null) continue
            // Drawer panel: parent owns the child's LayoutParams.
            if (v.parent is DrawerLayout) continue
            val left = frames[base]
            val top = frames[base + 1]
            val w = frames[base + 2]
            val h = frames[base + 3]
            val existing = v.layoutParams
            val mlp = if (existing is ViewGroup.MarginLayoutParams) {
                existing
            } else {
                ViewGroup.MarginLayoutParams(w, h)
            }
            mlp.width = w
            mlp.height = h
            mlp.leftMargin = left
            mlp.topMargin = top
            mlp.rightMargin = 0
            mlp.bottomMargin = 0
            v.layoutParams = mlp
        }
    }
}
