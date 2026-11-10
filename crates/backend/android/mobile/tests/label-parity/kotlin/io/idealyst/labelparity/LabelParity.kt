package io.idealyst.labelparity

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Typeface
import android.text.SpannableString
import android.text.Spanned
import android.text.TextUtils
import android.text.style.AbsoluteSizeSpan
import android.text.style.BackgroundColorSpan
import android.text.style.ForegroundColorSpan
import android.text.style.StyleSpan
import android.text.style.TypefaceSpan
import android.text.style.UnderlineSpan
import android.util.Log
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.widget.TextView
import io.idealyst.runtime.RustLabel
import io.idealyst.runtime.RustViewFactory

/**
 * `RustLabel` against the `TextView` it replaced, case by case: the
 * `TextView` set up as the backend's text factory did, the label from the
 * factory now, both driven through the setters the style code calls. Each
 * pair must measure to the same size and draw the same pixels, content-sized
 * (`AT_MOST`) and at an exact width. A selectable text (`text(selectable =
 * true)`, `RustViewFactory.selectableText`) is held to a `TextView` set up
 * the same way with selection on (Android lays some selectable text out
 * differently from plain text: a single ellipsized line). Logs
 * `[label-parity]` lines for `run.sh`.
 */
object LabelParity {
    private interface Target {
        fun size(unit: Int, v: Float)
        fun face(tf: Typeface?, style: Int)
        fun color(c: Int)
        fun gravity(g: Int)
        fun maxLines(n: Int)
        fun ellipsize(t: TextUtils.TruncateAt?)
        fun shadow(r: Float, dx: Float, dy: Float, c: Int)
        fun pad(p: Int)
        fun text(t: CharSequence)
    }

    private class Tv(val v: TextView) : Target {
        override fun size(unit: Int, v: Float) = this.v.setTextSize(unit, v)
        override fun face(tf: Typeface?, style: Int) = v.setTypeface(tf, style)
        override fun color(c: Int) = v.setTextColor(c)
        override fun gravity(g: Int) { v.gravity = g }
        override fun maxLines(n: Int) { v.maxLines = n }
        override fun ellipsize(t: TextUtils.TruncateAt?) { v.ellipsize = t }
        override fun shadow(r: Float, dx: Float, dy: Float, c: Int) = v.setShadowLayer(r, dx, dy, c)
        override fun pad(p: Int) = v.setPadding(p, p, p, p)
        override fun text(t: CharSequence) { v.text = t }
    }

    private class Rl(val v: RustLabel) : Target {
        override fun size(unit: Int, v: Float) = this.v.setTextSize(unit, v)
        override fun face(tf: Typeface?, style: Int) = v.setTypeface(tf, style)
        override fun color(c: Int) = v.setTextColor(c)
        override fun gravity(g: Int) = v.setGravity(g)
        override fun maxLines(n: Int) = v.setMaxLines(n)
        override fun ellipsize(t: TextUtils.TruncateAt?) = v.setEllipsize(t)
        override fun shadow(r: Float, dx: Float, dy: Float, c: Int) = v.setShadowLayer(r, dx, dy, c)
        override fun pad(p: Int) = v.setPadding(p, p, p, p)
        override fun text(t: CharSequence) = v.setText(t)
    }

    /** The `TextView` the backend's text factory built before `RustLabel`. */
    private fun textView(context: Context, content: String): TextView {
        val v = TextView(context)
        v.text = content
        v.includeFontPadding = false
        v.setLineSpacing(0f, 1f)
        v.minHeight = 0
        v.minimumHeight = 0
        v.gravity = Gravity.CENTER_VERTICAL or Gravity.START
        v.importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_AUTO
        v.setPadding(0, 0, 0, 0)
        return v
    }

    private fun spans() = SpannableString("Bold, red, mono, big and under runs").apply {
        setSpan(StyleSpan(Typeface.BOLD), 0, 4, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        setSpan(ForegroundColorSpan(Color.RED), 6, 9, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        setSpan(TypefaceSpan("monospace"), 11, 15, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        setSpan(AbsoluteSizeSpan(20, true), 17, 20, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        setSpan(BackgroundColorSpan(0x4400AAFF), 17, 20, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        setSpan(UnderlineSpan(), 25, 30, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
    }

    private const val LONG =
        "A longer paragraph of text that has to wrap onto several lines inside the column, gjpqy."

    private val cases: List<Pair<String, (Target, Float) -> Unit>> = listOf(
        "theme defaults" to { _, _ -> },
        "14dp" to { t, _ -> t.size(TypedValue.COMPLEX_UNIT_DIP, 14f) },
        "22dp bold, real face" to { t, _ -> t.size(1, 22f); t.face(Typeface.DEFAULT, Typeface.BOLD) },
        "serif italic" to { t, _ -> t.face(Typeface.create("serif", 0), Typeface.ITALIC) },
        "synthesized bold" to { t, _ -> t.face(Typeface.create("monospace", 0), Typeface.BOLD) },
        "typeface reset" to { t, _ -> t.face(Typeface.DEFAULT_BOLD, 0); t.face(null, 0) },
        "colour" to { t, _ -> t.color(0xFF2255CC.toInt()) },
        "wrap" to { t, _ -> t.text(LONG) },
        "3 lines, no ellipsis" to { t, _ -> t.text(LONG); t.maxLines(3) },
        "2 lines + ellipsis" to { t, _ -> t.text(LONG); t.maxLines(2); t.ellipsize(TextUtils.TruncateAt.END) },
        "1 line + ellipsis" to { t, _ -> t.text(LONG); t.maxLines(1); t.ellipsize(TextUtils.TruncateAt.END) },
        "centre" to { t, _ -> t.text("Centred, wrapping onto a second line"); t.gravity(Gravity.CENTER_VERTICAL or Gravity.CENTER_HORIZONTAL) },
        "right (gravity RIGHT)" to { t, _ -> t.text("Right aligned text that wraps"); t.gravity(Gravity.CENTER_VERTICAL or Gravity.RIGHT) },
        "left (gravity LEFT), RTL text" to { t, _ -> t.text("مرحبا بالعالم، هذا نص"); t.gravity(Gravity.CENTER_VERTICAL or Gravity.LEFT) },
        "start, RTL text" to { t, _ -> t.text("مرحبا بالعالم، هذا نص") },
        "mixed scripts" to { t, _ -> t.text("Café 你好 👋 مرحبا Ελλάδα") },
        "spans" to { t, _ -> t.text(spans()) },
        "shadow" to { t, d -> t.size(1, 20f); t.shadow(4 * d, 2 * d, 2 * d, 0x88000000.toInt()) },
        "padding 8dp" to { t, d -> t.pad((8 * d).toInt()) },
        "newlines" to { t, _ -> t.text("first line\nsecond line\n\nfourth") },
        "empty" to { t, _ -> t.text("") },
        "32dp descenders" to { t, _ -> t.text("Typography gjpqy"); t.size(1, 32f) },
    )

    private fun spec(size: Int, mode: Int) = View.MeasureSpec.makeMeasureSpec(size, mode)

    private fun render(v: View, wSpec: Int): Pair<String, IntArray?> {
        v.measure(wSpec, spec(0, View.MeasureSpec.UNSPECIFIED))
        val w = v.measuredWidth
        val h = v.measuredHeight
        v.layout(0, 0, w, h)
        if (w <= 0 || h <= 0) return "${w}x$h" to null
        val bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
        v.draw(Canvas(bmp))
        val pixels = IntArray(w * h)
        bmp.getPixels(pixels, 0, w, 0, 0, w, h)
        bmp.recycle()
        return "${w}x$h" to pixels
    }

    /** Runs every case; returns the number of failures. */
    @JvmStatic
    fun run(context: Context): Int {
        val d = context.resources.displayMetrics.density
        val column = (320 * d).toInt()
        var failures = 0
        for ((name, apply) in cases) {
            for ((mode, wSpec, selectable) in listOf(
                Triple("content", spec(column, View.MeasureSpec.AT_MOST), false),
                Triple("exact", spec(column, View.MeasureSpec.EXACTLY), false),
                Triple("content, selectable", spec(column, View.MeasureSpec.AT_MOST), true),
                Triple("exact, selectable", spec(column, View.MeasureSpec.EXACTLY), true),
            )) {
                val tv = textView(context, "Person 0-0 Lastname")
                if (selectable) tv.setTextIsSelectable(true)
                apply(Tv(tv), d)
                val made: View = if (selectable) {
                    RustViewFactory.selectableText(context, "Person 0-0 Lastname").also { apply(Tv(it), d) }
                } else {
                    RustViewFactory.text(context, "Person 0-0 Lastname").also { apply(Rl(it), d) }
                }
                val (tvSize, tvPx) = render(tv, wSpec)
                val (rlSize, rlPx) = render(made, wSpec)
                val differing = if (tvPx != null && rlPx != null && tvPx.size == rlPx.size) {
                    tvPx.indices.count { tvPx[it] != rlPx[it] }
                } else if (tvPx == null && rlPx == null) 0 else -1
                val ok = tvSize == rlSize && differing == 0
                if (!ok) failures++
                Log.i(
                    "idealyst",
                    "[label-parity] ${if (ok) "PASS" else "FAIL"} $name ($mode): " +
                        "textview $tvSize, label $rlSize, differing pixels " +
                        if (differing < 0) "n/a (sizes differ)" else "$differing",
                )
            }
        }
        Log.i("idealyst", "[label-parity] DONE ${cases.size * 4} checks, $failures failed")
        return failures
    }
}
