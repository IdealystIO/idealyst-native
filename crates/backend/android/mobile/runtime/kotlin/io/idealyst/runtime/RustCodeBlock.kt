package io.idealyst.runtime

import android.content.Context
import android.content.res.Resources
import android.graphics.Typeface
import android.text.SpannableString
import android.text.Spanned
import android.text.style.ForegroundColorSpan
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.HorizontalScrollView
import android.widget.TextView

/**
 * Single-node code-block widget. A [HorizontalScrollView] wrapping
 * exactly one [TextView] that uses a [SpannableString] to color each
 * syntax-highlighted token. Mirrors the iOS backend's
 * `UIScrollView` + `UILabel` + `NSAttributedString` setup, which in
 * turn mirrors web's `<pre>` + child `<span>`s.
 *
 * Why this exists: the framework's old per-token "one TextView per
 * span" path inflated a 15-line snippet into ~140 native nodes (one
 * TextView per highlighted token, one row View per line). On a docs
 * page with 4–5 code panels, that's 500–700 extra nodes — most of
 * Taffy's layout-compute budget and most of the per-view JNI apply-
 * frame budget. SpannableString collapses every token range into a
 * single TextView, so a code block costs **one** node regardless of
 * token count.
 *
 * Lifecycle: the Rust side constructs one instance per `code_block(…)`
 * call and feeds it spans via [update]. The TextView keeps the same
 * font / padding / scroll affordance across updates; only its
 * `text` (the SpannableString) gets rebuilt.
 */
class RustCodeBlock(context: Context) : HorizontalScrollView(context) {
    companion object {
        /// Inner padding (in dp) drawn around the code text. Matches
        /// the framework's `<pre>`-style inset on web and the iOS
        /// handler's UIScrollView contentInset; kept here so a future
        /// API setter can override per-instance without touching the
        /// Rust side.
        private const val PADDING_DP: Float = 20f
    }

    private val textView: TextView

    init {
        // The TextView holds the whole code block. Monospace ties to
        // the typeface authors expect for code; `Typeface.MONOSPACE`
        // is Android's built-in alias for the platform default
        // monospace face (typically Droid Sans Mono / Roboto Mono).
        textView = TextView(context).apply {
            typeface = Typeface.MONOSPACE
            // Match iOS UILabel's tighter text metrics — see
            // [[project_android_textview_fontpadding]]. Without this
            // the line-height visually leaks above and below glyphs
            // and the block looks taller than its web counterpart.
            includeFontPadding = false
            // The Rust side wires per-segment colors via a
            // SpannableString — leave the default text color
            // alone (it acts as the "default" when no span covers a
            // range, but in practice every range is covered).
            //
            // Inner padding lives on the TextView so it SCROLLS with
            // the text: when the user pans horizontally, the right
            // padding stays flush with the right edge of the visible
            // scroll area (same as `<pre> { padding: 20px;
            // overflow-x: auto }` on web). Padding on the outer
            // HorizontalScrollView would clip the content under a
            // fixed pad on either edge instead.
            val padPx = (PADDING_DP * Resources.getSystem().displayMetrics.density).toInt()
            setPadding(padPx, padPx, padPx, padPx)
        }
        // Fill our scroll viewport horizontally so the scroll view
        // gives the TextView unbounded width on the main axis; the
        // TextView's intrinsic size (sum of glyph widths per line)
        // drives the scroll content area.
        addView(
            textView,
            LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ).apply { gravity = Gravity.TOP }
        )

        // Single-line off — the TextView is multi-line. Padding is
        // applied by the Rust side via the framework's style system
        // landing on the outer scroll view container. Horizontal
        // scrollbars off keeps the visual minimal (Android's default
        // strip looks like a stripe under the code).
        isHorizontalScrollBarEnabled = false
    }

    /**
     * Replace the rendered text + color spans. Atomic — the TextView
     * sees the new SpannableString in one assignment so a mid-update
     * tap can't catch a half-formed state.
     *
     * `text` is the full source string (with `\n`s preserved). For
     * each `i`, `starts[i]..ends[i]` is the byte range within `text`
     * that gets `colors[i]` (as an ARGB int in the same byte layout
     * `Color.argb` produces). All three arrays must have the same
     * length; the Rust caller is responsible for the contract.
     *
     * `ForegroundColorSpan` is the cheapest of Spannable's color
     * spans — no per-span object beyond a wrapper around an `int`,
     * and Android's text engine applies them inline without a per-
     * span layout step. So even a 500-token block costs ~500 small
     * objects + one `setText` call rather than 500 TextViews.
     */
    fun update(text: String, starts: IntArray, ends: IntArray, colors: IntArray) {
        require(starts.size == ends.size && ends.size == colors.size) {
            "starts/ends/colors must have matching lengths " +
                "(${starts.size} / ${ends.size} / ${colors.size})"
        }
        val span = SpannableString(text)
        for (i in starts.indices) {
            val start = starts[i].coerceIn(0, text.length)
            val end = ends[i].coerceIn(start, text.length)
            if (end == start) continue
            // SPAN_EXCLUSIVE_EXCLUSIVE = "don't extend the span if
            // adjacent text changes" — irrelevant here (we replace
            // the whole text on each update), but the most defensible
            // default for a static highlight.
            span.setSpan(
                ForegroundColorSpan(colors[i]),
                start,
                end,
                Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
            )
        }
        textView.text = span
    }

    /**
     * Override the per-block text size in scaled pixels. Called from
     * the Rust handler so authors can change codeblock typography via
     * the SDK style; we don't pull it from the framework's style
     * system because the style lands on the outer scroll view, not
     * the TextView. Default is the platform `TextView` default
     * (currently 14sp on stock Android).
     */
    fun setFontSizeSp(sizeSp: Float) {
        textView.setTextSize(android.util.TypedValue.COMPLEX_UNIT_SP, sizeSp)
    }
}
