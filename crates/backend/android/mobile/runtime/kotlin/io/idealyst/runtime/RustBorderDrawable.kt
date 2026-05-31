package io.idealyst.runtime

import android.graphics.Canvas
import android.graphics.ColorFilter
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PixelFormat
import android.graphics.Rect
import android.graphics.RectF
import android.graphics.drawable.Drawable

/**
 * Custom Drawable that paints per-side borders (top / right / bottom /
 * left) with independent widths + colors, and respects per-corner
 * border-radius so corners blend cleanly with the rounded background
 * GradientDrawable underneath. Set as a View's `foreground` so it
 * renders on top of the View's content + children.
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

    /// Per-corner radii in px, in order: tl, tr, br, bl. Updated via
    /// [setCornerRadii] so the corner curves of the border match the
    /// rounded background `GradientDrawable` underneath. `0f` on any
    /// corner = square corner there. Defaults to all-zero so a view
    /// without border-radius still paints crisp 90° corners.
    private var radiusTL: Float = 0f
    private var radiusTR: Float = 0f
    private var radiusBR: Float = 0f
    private var radiusBL: Float = 0f

    private val paint = Paint().apply {
        style = Paint.Style.STROKE
        isAntiAlias = true
    }
    private val path = Path()
    private val rect = RectF()

    /**
     * Bulk setter for the per-side widths + colors. Widths in px,
     * colors as packed ARGB ints. Atomic update so the next draw
     * sees a consistent state.
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

    /**
     * Per-corner radii in px (tl, tr, br, bl). Called by the Rust
     * apply-style path whenever `border_*_radius` changes, in the
     * same order GradientDrawable's `setCornerRadii` uses (so the
     * caller can pass the same array).
     */
    fun setCornerRadii(tl: Float, tr: Float, br: Float, bl: Float) {
        radiusTL = tl
        radiusTR = tr
        radiusBR = br
        radiusBL = bl
        invalidateSelf()
    }

    override fun draw(canvas: Canvas) {
        val b = bounds
        if (b.isEmpty) return
        // Common-case fast path: all four sides share the same width
        // AND the same color. Paint the whole frame as a single
        // round-rect stroke — cheaper than four arcs, and produces
        // a visually continuous border the way CSS authors expect.
        if (topWidth > 0
            && topWidth == rightWidth
            && rightWidth == bottomWidth
            && bottomWidth == leftWidth
            && topColor == rightColor
            && rightColor == bottomColor
            && bottomColor == leftColor) {
            val w = topWidth.toFloat()
            // Inset by half-stroke so the stroke sits on the
            // GradientDrawable's edge instead of straddling it
            // (which would bleed half-width outside the rounded
            // background and look like a doubled border).
            val half = w / 2f
            rect.set(
                b.left + half,
                b.top + half,
                b.right - half,
                b.bottom - half,
            )
            paint.color = topColor
            paint.strokeWidth = w
            // Use the largest radius — when all 4 corners share a
            // radius this is exact; for mixed corners with uniform
            // border the result still looks closer to "rounded
            // border" than per-side rectangles.
            val maxR = maxOf(radiusTL, radiusTR, radiusBR, radiusBL)
            val r = maxOf(0f, maxR - half)
            canvas.drawRoundRect(rect, r, r, paint)
            return
        }
        // Mixed-side fallback: clip each edge stroke into a quadrant-
        // sized region so corners cleanly meet — and skip the corner
        // bands so the rounded background shows through (rather than
        // a square corner peeking past the radius).
        if (topWidth > 0) drawEdge(canvas, Edge.TOP)
        if (rightWidth > 0) drawEdge(canvas, Edge.RIGHT)
        if (bottomWidth > 0) drawEdge(canvas, Edge.BOTTOM)
        if (leftWidth > 0) drawEdge(canvas, Edge.LEFT)
    }

    private enum class Edge { TOP, RIGHT, BOTTOM, LEFT }

    private fun drawEdge(canvas: Canvas, edge: Edge) {
        val b = bounds
        paint.style = Paint.Style.FILL
        when (edge) {
            Edge.TOP -> {
                paint.color = topColor
                val left = b.left + maxOf(radiusTL, leftWidth.toFloat())
                val right = b.right - maxOf(radiusTR, rightWidth.toFloat())
                if (right > left) {
                    canvas.drawRect(
                        left, b.top.toFloat(),
                        right, (b.top + topWidth).toFloat(),
                        paint,
                    )
                }
            }
            Edge.BOTTOM -> {
                paint.color = bottomColor
                val left = b.left + maxOf(radiusBL, leftWidth.toFloat())
                val right = b.right - maxOf(radiusBR, rightWidth.toFloat())
                if (right > left) {
                    canvas.drawRect(
                        left, (b.bottom - bottomWidth).toFloat(),
                        right, b.bottom.toFloat(),
                        paint,
                    )
                }
            }
            Edge.LEFT -> {
                paint.color = leftColor
                val top = b.top + maxOf(radiusTL, topWidth.toFloat())
                val bottom = b.bottom - maxOf(radiusBL, bottomWidth.toFloat())
                if (bottom > top) {
                    canvas.drawRect(
                        b.left.toFloat(), top,
                        (b.left + leftWidth).toFloat(), bottom,
                        paint,
                    )
                }
            }
            Edge.RIGHT -> {
                paint.color = rightColor
                val top = b.top + maxOf(radiusTR, topWidth.toFloat())
                val bottom = b.bottom - maxOf(radiusBR, bottomWidth.toFloat())
                if (bottom > top) {
                    canvas.drawRect(
                        (b.right - rightWidth).toFloat(), top,
                        b.right.toFloat(), bottom,
                        paint,
                    )
                }
            }
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
