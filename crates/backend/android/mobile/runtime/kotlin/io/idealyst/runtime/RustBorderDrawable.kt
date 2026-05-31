package io.idealyst.runtime

import android.graphics.Canvas
import android.graphics.ColorFilter
import android.graphics.Paint
import android.graphics.PixelFormat
import android.graphics.Rect
import android.graphics.drawable.Drawable

/**
 * Custom Drawable that paints per-side borders (top / right / bottom /
 * left) with independent widths + colors. Set as a View's
 * `foreground` so it renders on top of the View's content + children.
 *
 * Android's built-in `GradientDrawable.setStroke(width, color)` only
 * supports a uniform border on all four sides — the framework's
 * `border_{top,right,bottom,left}_*` style props let authors pick
 * individual sides (CSS-style), so we paint them ourselves. Mirrors
 * the iOS backend's per-side `UIView` subview approach
 * (`install_border_side` in `backend-ios-core/src/style.rs`); on
 * Android a custom Drawable is cleaner than adding chrome subviews
 * because:
 *
 *   - No interference with Taffy's child tracking (no extra Views in
 *     the hierarchy for the layout pass to register, no
 *     `apply_frame_to_layout_params` overrides needed).
 *   - No hit-test conflicts — Drawable.draw is paint-only.
 *   - Resizes automatically with the View — `onBoundsChange` is the
 *     hook the framework already wires up via `setBounds`.
 *
 * All measurements are in device pixels (caller has already converted
 * from dp). Colors are packed ARGB ints.
 *
 * Recycling: the framework keeps one instance per styled view alive
 * across re-applies and mutates it in place via the `update*`
 * setters — re-allocating per apply would churn the GC.
 */
class RustBorderDrawable : Drawable() {
    private var topWidth: Int = 0
    private var rightWidth: Int = 0
    private var bottomWidth: Int = 0
    private var leftWidth: Int = 0

    private var topColor: Int = 0
    private var rightColor: Int = 0
    private var bottomColor: Int = 0
    private var leftColor: Int = 0

    private val paint = Paint().apply {
        style = Paint.Style.FILL
        isAntiAlias = false
    }

    /**
     * Bulk setter — atomic update for all four sides so the next
     * draw sees a consistent state. Widths are in device pixels;
     * colors are packed ARGB ints (Color.argb byte layout).
     */
    fun update(
        topW: Int, topC: Int,
        rightW: Int, rightC: Int,
        bottomW: Int, bottomC: Int,
        leftW: Int, leftC: Int,
    ) {
        topWidth = topW; topColor = topC
        rightWidth = rightW; rightColor = rightC
        bottomWidth = bottomW; bottomColor = bottomC
        leftWidth = leftW; leftColor = leftC
        invalidateSelf()
    }

    override fun draw(canvas: Canvas) {
        val b: Rect = bounds
        // Each side is painted as a full-width / full-height rect on
        // that edge. Adjacent sides overlap at the corners — last
        // painter wins. Order (top, bottom, left, right) keeps
        // horizontal edges underneath vertical edges so a 1px corner
        // reads as the side color the eye expects (vertical sides
        // tend to look "cleaner" at the corner crossover).
        if (topWidth > 0) {
            paint.color = topColor
            canvas.drawRect(
                b.left.toFloat(),
                b.top.toFloat(),
                b.right.toFloat(),
                (b.top + topWidth).toFloat(),
                paint,
            )
        }
        if (bottomWidth > 0) {
            paint.color = bottomColor
            canvas.drawRect(
                b.left.toFloat(),
                (b.bottom - bottomWidth).toFloat(),
                b.right.toFloat(),
                b.bottom.toFloat(),
                paint,
            )
        }
        if (leftWidth > 0) {
            paint.color = leftColor
            canvas.drawRect(
                b.left.toFloat(),
                b.top.toFloat(),
                (b.left + leftWidth).toFloat(),
                b.bottom.toFloat(),
                paint,
            )
        }
        if (rightWidth > 0) {
            paint.color = rightColor
            canvas.drawRect(
                (b.right - rightWidth).toFloat(),
                b.top.toFloat(),
                b.right.toFloat(),
                b.bottom.toFloat(),
                paint,
            )
        }
    }

    override fun setAlpha(alpha: Int) {
        // Per-side colors already encode their alpha. Drawable's
        // global alpha is rarely useful for borders; ignore.
    }

    override fun setColorFilter(colorFilter: ColorFilter?) {
        paint.colorFilter = colorFilter
        invalidateSelf()
    }

    @Suppress("OVERRIDE_DEPRECATION")
    override fun getOpacity(): Int = PixelFormat.TRANSLUCENT
}
