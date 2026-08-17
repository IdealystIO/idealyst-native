package io.idealyst.runtime

import android.graphics.Canvas
import android.graphics.DashPathEffect
import android.graphics.Paint
import android.text.Layout
import android.text.Spanned
import android.text.style.LineBackgroundSpan
import android.widget.TextView
import kotlin.math.max
import kotlin.math.min

/**
 * A per-range underline with its own line pattern and colour — the
 * Android realization of `runtime_shared::styled_text::RunUnderline`.
 *
 * Why a custom span at all: `android.text.style.UnderlineSpan` is the
 * only built-in underline, and it draws a **solid line in the run's own
 * text colour** with no way to change either. The framework's run
 * underline carries a pattern (solid / dotted / dashed) and an optional
 * independent colour, because that is what the other backends draw
 * natively — CSS `text-decoration-style` + `-color` on web, the
 * `NSUnderlineStylePattern*` bits plus `NSUnderlineColor` on Apple.
 * Leaving Android on `UnderlineSpan` would mean a red dotted diagnostic
 * mark rendering as a solid line in the text colour, i.e. the same
 * authored style meaning something different per platform — the one
 * thing CLAUDE.md §7 rules out. Spannable has no attribute for this, so
 * the only route is drawing it ourselves.
 *
 * [LineBackgroundSpan] (rather than a `CharacterStyle`) because it is
 * the only span hook that hands us a [Canvas]: `CharacterStyle` can
 * only mutate the `TextPaint`, and a `PathEffect` on the paint does not
 * reach the underline, which Skia fills as a plain rect. The callback
 * fires once per LINE of the paragraph, which is also what makes this
 * correct across soft wraps: a span covering three wrapped lines gets
 * three draw calls and we underline each line's slice of it.
 *
 * Horizontal extents come from the host [TextView]'s [Layout], not from
 * `Paint.measureText`: `measureText` uses the base paint and drifts
 * wherever another span changes the advance widths (a larger
 * `AbsoluteSizeSpan`, a bold `StyleSpan`). The `Layout` measured the
 * real, fully-spanned text, so `getPrimaryHorizontal` is exact. The
 * view is read at DRAW time rather than captured once, because a span
 * is built before the text is set and the layout only exists after the
 * first measure pass; before then we fall back to measuring, which is
 * exact for the uniform-metric case that dominates — code.
 *
 * The span holds its host view and the view's text holds the span: a
 * reference cycle, and a deliberate one. It is confined to the JVM heap
 * where the collector handles cycles, and it is what keeps the span
 * self-sufficient — the alternative (Rust pushing the layout in after
 * every `setText`) silently draws with stale offsets whenever a layout
 * pass happens that Rust did not trigger.
 *
 * @param textView the view this span is drawn into.
 * @param color ARGB line colour, or [INHERIT_COLOR] to follow the text.
 * @param pattern one of [PATTERN_SOLID], [PATTERN_DOTTED], [PATTERN_DASHED].
 */
class RustUnderlineSpan(
    private val textView: TextView,
    private val color: Int,
    private val pattern: Int,
) : LineBackgroundSpan {

    private val paintCopy = Paint()

    override fun drawBackground(
        canvas: Canvas,
        paint: Paint,
        left: Int,
        right: Int,
        top: Int,
        baseline: Int,
        bottom: Int,
        text: CharSequence,
        lineStart: Int,
        lineEnd: Int,
        lineNumber: Int,
    ) {
        val spanned = text as? Spanned ?: return
        // Our own range within the paragraph. `getSpanStart` on the
        // CharSequence is the only way to learn it — LineBackgroundSpan
        // is told about the LINE, not about itself.
        val spanStart = spanned.getSpanStart(this)
        val spanEnd = spanned.getSpanEnd(this)
        if (spanStart < 0 || spanEnd <= spanStart) return

        // Intersect with this line; nothing to draw on lines the span
        // does not reach.
        val from = max(spanStart, lineStart)
        val to = min(spanEnd, lineEnd)
        if (from >= to) return

        val l = textView.layout
        val startX: Float
        val endX: Float
        if (l != null) {
            startX = l.getPrimaryHorizontal(from)
            // `getPrimaryHorizontal(to)` at a line break returns the
            // START of the next line, so clamp the right edge to this
            // line's own measured width.
            endX = if (to >= lineEnd) l.getLineRight(lineNumber) else l.getPrimaryHorizontal(to)
        } else {
            startX = left + paint.measureText(text, lineStart, from)
            endX = left + paint.measureText(text, lineStart, to)
        }
        if (endX <= startX) return

        // Sit the line just below the baseline, thickness scaled off the
        // font so it tracks the text size instead of being a fixed dp
        // that looks hairline at 24sp and clumsy at 10sp.
        val thickness = max(1f, paint.textSize * THICKNESS_RATIO)
        val y = baseline + max(1f, paint.textSize * BASELINE_GAP_RATIO)

        paintCopy.set(paint)
        paintCopy.style = Paint.Style.STROKE
        paintCopy.strokeWidth = thickness
        paintCopy.pathEffect = when (pattern) {
            // Dash lengths are expressed in multiples of the stroke
            // width so the pattern stays visually constant across text
            // sizes — a fixed-px dash turns into a solid line at large
            // sizes and disappears at small ones.
            PATTERN_DOTTED -> DashPathEffect(
                floatArrayOf(thickness * DOT_ON, thickness * DOT_OFF), 0f
            )
            PATTERN_DASHED -> DashPathEffect(
                floatArrayOf(thickness * DASH_ON, thickness * DASH_OFF), 0f
            )
            else -> null
        }
        if (color != INHERIT_COLOR) {
            paintCopy.color = color
        }
        canvas.drawLine(startX, y, endX, y, paintCopy)
    }

    companion object {
        /** Sentinel: draw in the text's own colour. */
        const val INHERIT_COLOR: Int = 0

        const val PATTERN_SOLID: Int = 0
        const val PATTERN_DOTTED: Int = 1
        const val PATTERN_DASHED: Int = 2

        // Geometry ratios MIRROR `runtime_shared::styled_text::
        // underline_geometry` — the GPU renderer draws its underline
        // rects from the same numbers, so a dotted mark is the same
        // mark on both backends that hand-draw it. Change them there
        // first, then here.
        private const val THICKNESS_RATIO = 0.07f
        private const val BASELINE_GAP_RATIO = 0.12f
        private const val DOT_ON = 1.0f
        private const val DOT_OFF = 1.5f
        private const val DASH_ON = 3.0f
        private const val DASH_OFF = 2.5f
    }
}
