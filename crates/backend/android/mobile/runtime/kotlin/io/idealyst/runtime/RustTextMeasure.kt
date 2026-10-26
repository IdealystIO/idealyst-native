package io.idealyst.runtime

import android.text.Layout
import android.text.TextPaint
import android.widget.TextView
import kotlin.math.ceil

/**
 * Text sizing that `View.measure` cannot answer by itself.
 */
object RustTextMeasure {
    /**
     * The text's CSS min-content width in px: its widest unbreakable run
     * (a word, split on whitespace) plus the view's horizontal padding.
     *
     * The layout's min-content probe used to measure at `AT_MOST 0`, where
     * `TextView` breaks inside words and wraps one character a line — a
     * tall column of single glyphs whose height can leak into a parent's
     * size. Web min-content wraps between words, and this is that width.
     * Each run is measured with `Layout.getDesiredWidth` over the view's
     * own text, so spans (a bold run, a larger run) count.
     *
     * Returns 0 for a single-line view (`maxLines == 1`): it truncates
     * rather than wraps, so its narrowest is the ellipsis, which the
     * `AT_MOST 0` probe already gives.
     */
    @JvmStatic
    fun minContentWidth(view: TextView): Int {
        if (view.maxLines == 1) return 0
        val text: CharSequence = view.text ?: return 0
        val paint: TextPaint = view.paint
        var widest = 0f
        var start = -1
        for (i in 0..text.length) {
            val boundary = i == text.length || text[i].isWhitespace()
            if (boundary) {
                if (start >= 0) {
                    widest = maxOf(widest, Layout.getDesiredWidth(text, start, i, paint))
                    start = -1
                }
            } else if (start < 0) {
                start = i
            }
        }
        return ceil(widest).toInt() + view.compoundPaddingLeft + view.compoundPaddingRight
    }
}
