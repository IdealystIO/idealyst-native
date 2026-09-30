package io.idealyst.runtime

import android.graphics.Canvas
import android.graphics.ColorFilter
import android.graphics.DashPathEffect
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
 *
 * Dashed / dotted borders (`border_style`, set via [setBorderStyle]):
 * this drawable owns them rather than `GradientDrawable.setStroke(w, c,
 * dashWidth, dashGap)`, because that API (a) is uniform-only, like the
 * solid stroke, so per-side dashed borders would be impossible; (b)
 * cannot set a round cap, so dots would be square; and (c) takes a
 * fixed gap, so the pattern would not close cleanly round the box. The
 * pattern must instead be FITTED to the path, and the path is these
 * bounds — known only here, and changing on every layout. So when the
 * geometry is dirty [draw] asks Rust for it (`nativeLoopDash` /
 * `nativeSideDash`, backed by `runtime_shared::border_dash` via
 * `border_dash_policy.rs`) and draws exactly the numbers returned. This
 * file deliberately holds NO dash ratios: every native backend takes
 * them from `border_dash.rs`, so there is nothing here to drift.
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

    /// One of the `STYLE_*` codes. [STYLE_SOLID] draws exactly the
    /// pre-`border_style` border via [paint]; the patterned styles use
    /// [dashPaint], so a dash [DashPathEffect] can never leak onto a
    /// solid stroke after a dashed → solid re-apply.
    private var borderStyle: Int = STYLE_SOLID
    private val dashPaint = Paint().apply {
        style = Paint.Style.STROKE
        isAntiAlias = true
    }
    /// Cached fitted geometry. Invalidated by anything that changes the
    /// path or the pattern: bounds ([onBoundsChange]), widths/colours
    /// ([update] — including every tick of `Animators.animateBorder`, so
    /// an animated width re-fits its dashes), radii, style.
    private var geometryDirty = true
    private var loopGeometry: FloatArray? = null
    private val sideGeometry = arrayOfNulls<FloatArray>(4)
    /// Effects built with the geometry, not per draw (no allocation in
    /// `draw`); index 0..3 = sides, [LOOP] = the uniform loop.
    private val effects = arrayOfNulls<DashPathEffect>(5)
    private val dashPath = Path()
    /// Per-side centrelines as Paths rather than `drawLine` calls, so
    /// sides and loop both take the general path-stroking route that
    /// applies the PathEffect (`drawLine` can be special-cased by the
    /// renderer as a plain line primitive).
    private val sidePaths = Array(4) { Path() }
    private val arcRect = RectF()

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
        geometryDirty = true
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
        geometryDirty = true
        invalidateSelf()
    }

    /**
     * Line pattern for every side: [STYLE_SOLID], [STYLE_DASHED] or
     * [STYLE_DOTTED]. Called by the Rust apply-style path when
     * `border_style` changes (and once for each new drawable).
     */
    fun setBorderStyle(style: Int) {
        if (style == borderStyle) return
        borderStyle = style
        geometryDirty = true
        invalidateSelf()
    }

    override fun onBoundsChange(bounds: Rect) {
        super.onBoundsChange(bounds)
        geometryDirty = true
    }

    private fun isUniform(): Boolean =
        topWidth > 0
            && topWidth == rightWidth
            && rightWidth == bottomWidth
            && bottomWidth == leftWidth
            && topColor == rightColor
            && rightColor == bottomColor
            && bottomColor == leftColor

    override fun draw(canvas: Canvas) {
        val b = bounds
        if (b.isEmpty) return
        if (borderStyle != STYLE_SOLID) {
            drawPatterned(canvas, b)
            return
        }
        // Common-case fast path: all four sides share the same width
        // AND the same color. Paint the whole frame as a single
        // round-rect stroke — cheaper than four arcs, and produces
        // a visually continuous border the way CSS authors expect.
        if (isUniform()) {
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
            // `drawEdge` switches the shared paint to FILL; without
            // resetting it, a border that went asymmetric → uniform
            // filled the whole box in the border colour.
            paint.style = Paint.Style.STROKE
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

    /**
     * Dashed / dotted border. Same uniform-vs-per-side split as the
     * solid path: a uniform border is one loop following the corner
     * radii (per corner — the shared `LoopPath` geometry, which every
     * backend draws); otherwise each side with a width is a straight
     * patterned line in its own colour.
     */
    private fun drawPatterned(canvas: Canvas, b: Rect) {
        val uniform = isUniform()
        if (geometryDirty) {
            val bw = b.width().toFloat()
            val bh = b.height().toFloat()
            if (uniform) {
                loopGeometry = nativeLoopDash(
                    borderStyle, topWidth.toFloat(), bw, bh,
                    radiusTL, radiusTR, radiusBR, radiusBL,
                )
                sideGeometry.fill(null)
                effects.fill(null)
                loopGeometry?.let {
                    buildLoopPath(it)
                    effects[LOOP] = DashPathEffect(floatArrayOf(it[8], it[9]), it[10])
                }
            } else {
                loopGeometry = null
                effects.fill(null)
                val widths = intArrayOf(topWidth, rightWidth, bottomWidth, leftWidth)
                for (side in 0 until 4) {
                    val g = if (widths[side] > 0) {
                        nativeSideDash(borderStyle, side, widths[side].toFloat(), bw, bh)
                    } else {
                        null
                    }
                    sideGeometry[side] = g
                    if (g != null) {
                        effects[side] = DashPathEffect(floatArrayOf(g[4], g[5]), g[6])
                        sidePaths[side].reset()
                        sidePaths[side].moveTo(g[0], g[1])
                        sidePaths[side].lineTo(g[2], g[3])
                    }
                }
            }
            geometryDirty = false
        }
        val save = canvas.save()
        canvas.translate(b.left.toFloat(), b.top.toFloat())
        if (uniform) {
            val g = loopGeometry
            if (g != null) {
                applyDash(topWidth.toFloat(), topColor, effects[LOOP], g[11])
                canvas.drawPath(dashPath, dashPaint)
            }
        } else {
            val widths = intArrayOf(topWidth, rightWidth, bottomWidth, leftWidth)
            val colors = intArrayOf(topColor, rightColor, bottomColor, leftColor)
            for (side in 0 until 4) {
                val g = sideGeometry[side] ?: continue
                applyDash(widths[side].toFloat(), colors[side], effects[side], g[7])
                canvas.drawPath(sidePaths[side], dashPaint)
            }
        }
        canvas.restoreToCount(save)
    }

    private fun applyDash(width: Float, color: Int, effect: DashPathEffect?, round: Float) {
        dashPaint.strokeWidth = width
        dashPaint.color = color
        // Dots come back as zero-length dashes; Skia draws those as a
        // dot only with a round cap (a butt cap draws nothing).
        dashPaint.strokeCap = if (round != 0f) Paint.Cap.ROUND else Paint.Cap.BUTT
        dashPaint.pathEffect = effect
    }

    /**
     * The centreline loop from `nativeLoopDash`'s
     * `[x, y, w, h, tl, tr, br, bl, ...]`, walked exactly as
     * `border_dash::LoopPath::point_at` walks it — clockwise from the
     * left end of the top edge — so the dash phase puts the first mark
     * where every other backend puts it. `Path.addRoundRect` is not
     * used: its start point and direction are Skia's choice, not ours.
     */
    private fun buildLoopPath(g: FloatArray) {
        val x0 = g[0]; val y0 = g[1]
        val x1 = g[0] + g[2]; val y1 = g[1] + g[3]
        val tl = g[4]; val tr = g[5]; val br = g[6]; val bl = g[7]
        dashPath.reset()
        dashPath.moveTo(x0 + tl, y0)
        dashPath.lineTo(x1 - tr, y0)
        if (tr > 0f) {
            arcRect.set(x1 - 2 * tr, y0, x1, y0 + 2 * tr)
            dashPath.arcTo(arcRect, -90f, 90f, false)
        }
        dashPath.lineTo(x1, y1 - br)
        if (br > 0f) {
            arcRect.set(x1 - 2 * br, y1 - 2 * br, x1, y1)
            dashPath.arcTo(arcRect, 0f, 90f, false)
        }
        dashPath.lineTo(x0 + bl, y1)
        if (bl > 0f) {
            arcRect.set(x0, y1 - 2 * bl, x0 + 2 * bl, y1)
            dashPath.arcTo(arcRect, 90f, 90f, false)
        }
        dashPath.lineTo(x0, y0 + tl)
        if (tl > 0f) {
            arcRect.set(x0, y0, x0 + 2 * tl, y0 + 2 * tl)
            dashPath.arcTo(arcRect, 180f, 90f, false)
        }
        dashPath.close()
    }

    /** Fitted uniform-loop geometry, or null for a solid style. See `border_dash_policy::loop_geometry`. */
    private external fun nativeLoopDash(
        style: Int, width: Float, boxW: Float, boxH: Float,
        tl: Float, tr: Float, br: Float, bl: Float,
    ): FloatArray?

    /** Fitted per-side geometry, or null for a solid style. See `border_dash_policy::side_geometry`. */
    private external fun nativeSideDash(
        style: Int, side: Int, width: Float, boxW: Float, boxH: Float,
    ): FloatArray?

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
        dashPaint.colorFilter = colorFilter
        invalidateSelf()
    }

    @Suppress("OVERRIDE_DEPRECATION")
    override fun getOpacity(): Int = PixelFormat.TRANSLUCENT

    companion object {
        // `border_style` wire codes. MIRROR `border_dash_policy.rs`'s
        // `STYLE_*` consts; `kotlin_style_codes_match_rust` there fails
        // if these drift.
        const val STYLE_SOLID: Int = 0
        const val STYLE_DASHED: Int = 1
        const val STYLE_DOTTED: Int = 2

        private const val LOOP = 4
    }
}
