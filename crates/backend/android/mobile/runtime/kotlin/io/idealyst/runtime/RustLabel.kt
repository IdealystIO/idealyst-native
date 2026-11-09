package io.idealyst.runtime

import android.content.Context
import android.graphics.Canvas
import android.graphics.Typeface
import android.os.Build
import android.text.BoringLayout
import android.text.Layout
import android.text.StaticLayout
import android.text.TextDirectionHeuristics
import android.text.TextPaint
import android.text.TextUtils
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityManager
import android.view.accessibility.AccessibilityNodeInfo
import android.widget.TextView
import java.util.WeakHashMap
import kotlin.math.ceil

/**
 * The `text` primitive: a `View` that lays out and draws its own text.
 *
 * A `TextView` is Android's whole text widget (editing, selection,
 * autofill, autosize, movement methods), and its constructor resolves the
 * theme's text style and sets all of that up for every label: 45–100 µs a
 * label on an emulator, most of the cost of mounting a list. A label needs a
 * paint and a laid-out paragraph, which this view builds itself (~10 µs),
 * caching the [Layout] per width so the layout engine's measure probes are
 * lookups.
 *
 * It renders as the `TextView` it replaces, pixel for pixel:
 *
 *  - Theme defaults (text size, typeface, colour, locale, font features,
 *    line breaking, fallback line spacing) are copied from one real
 *    `TextView` per theme and font scale ([Defaults]), so an unstyled label
 *    matches what the theme would have drawn.
 *  - [onMeasure] follows `TextView.onMeasure`: width = min(available,
 *    unwrapped width + padding), height = the layout's height at that width
 *    plus padding, the paragraph capped at [setMaxLines].
 *  - [onDraw] follows `TextView.onDraw`: clipped to the padding box (grown
 *    by the shadow), offset vertically by the gravity.
 *  - The backend's `TextView` setup is the default: no font padding, line
 *    spacing 1.0, no minimum height, gravity `CENTER_VERTICAL | START`.
 *
 * The setters the backend calls on a text node keep `TextView`'s names and
 * signatures (`setTextColor(int)` is also what `ObjectAnimator`'s
 * `"textColor"` property resolves), so the style code, fonts, animation and
 * introspection reach a label the way they reach a `TextView`.
 *
 * Accessibility reports the text and the `TextView` class name, which is
 * what TalkBack and `uiautomator` read from a label today.
 */
class RustLabel(context: Context) : View(context) {
    /** The paint every line is drawn with; spans layer over it. */
    val paint: TextPaint
    private var text: CharSequence = ""
    private var textColor: Int
    private var gravity = Gravity.CENTER_VERTICAL or Gravity.START
    private var maxLines = Int.MAX_VALUE
    private var ellipsize: TextUtils.TruncateAt? = null
    private var shadowRadius = 0f
    private var shadowDx = 0f
    private var shadowDy = 0f
    private val defaults: Defaults

    /** The layout drawn, at the last width asked for. */
    private var layout: Layout? = null
    /** Recent layouts by content width, newest last. The layout engine
     *  asks a label for its size at two or three widths a pass (min- and
     *  max-content, then the final width), so one cached width rebuilt
     *  the paragraph on alternate probes. */
    private val layouts = ArrayList<Pair<Int, Layout>>(LAYOUT_CACHE)
    /** The unwrapped width of [text] in px, or -1 until measured. */
    private var desiredWidth = -1

    init {
        defaults = Defaults.of(context)
        // `set(TextPaint)`, not the `TextPaint(Paint)` constructor: that
        // copies only `Paint`'s fields and drops `density`, which a dip
        // `AbsoluteSizeSpan` scales by (a 20dp run drew at 20px).
        paint = TextPaint().apply { set(defaults.paint) }
        textColor = defaults.color
        paint.color = textColor
    }

    // --- The TextView surface the backend calls ---------------------------

    fun setText(t: CharSequence?) {
        val changed = !TextUtils.equals(text, t ?: "")
        text = t ?: ""
        textChanged()
        if (changed) notifyTextChanged()
    }

    /**
     * What `TextView.setText` tells accessibility: the node's text changed.
     * It is how TalkBack refreshes the text it holds for this node and how
     * a `live_region` label's new value gets announced.
     */
    private fun notifyTextChanged() {
        val am = context.getSystemService(Context.ACCESSIBILITY_SERVICE) as? AccessibilityManager
        if (am == null || !am.isEnabled || !isAttachedToWindow) return
        @Suppress("DEPRECATION")
        val event = AccessibilityEvent.obtain(AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED)
        event.contentChangeTypes = AccessibilityEvent.CONTENT_CHANGE_TYPE_TEXT
        sendAccessibilityEventUnchecked(event)
    }

    fun getText(): CharSequence = text

    fun setTextColor(color: Int) {
        if (color == textColor) return
        textColor = color
        paint.color = color
        invalidate()
    }

    fun getCurrentTextColor(): Int = textColor

    fun setTextSize(unit: Int, size: Float) {
        val px = TypedValue.applyDimension(unit, size, resources.displayMetrics)
        if (px == paint.textSize) return
        paint.textSize = px
        textChanged()
    }

    fun getTextSize(): Float = paint.textSize

    /**
     * `TextView.setTypeface(Typeface, int)`: `style` asks for bold and/or
     * italic; whatever the face lacks is synthesized with fake bold and a
     * skew, exactly as `TextView` does.
     */
    fun setTypeface(tf: Typeface?, style: Int) {
        if (style > 0) {
            val face = if (tf == null) Typeface.defaultFromStyle(style) else Typeface.create(tf, style)
            setTypeface(face)
            val need = style and (face?.style ?: 0).inv()
            paint.isFakeBoldText = (need and Typeface.BOLD) != 0
            paint.textSkewX = if ((need and Typeface.ITALIC) != 0) -0.25f else 0f
        } else {
            paint.isFakeBoldText = false
            paint.textSkewX = 0f
            setTypeface(tf)
        }
        textChanged()
    }

    fun setTypeface(tf: Typeface?) {
        if (paint.typeface != tf) {
            paint.typeface = tf
            textChanged()
        }
    }

    fun getTypeface(): Typeface? = paint.typeface

    fun setGravity(g: Int) {
        var value = g
        if (value and Gravity.RELATIVE_HORIZONTAL_GRAVITY_MASK == 0) value = value or Gravity.START
        if (value and Gravity.VERTICAL_GRAVITY_MASK == 0) value = value or Gravity.TOP
        if (value == gravity) return
        val horizontalChanged = (value and Gravity.RELATIVE_HORIZONTAL_GRAVITY_MASK) !=
            (gravity and Gravity.RELATIVE_HORIZONTAL_GRAVITY_MASK)
        gravity = value
        if (horizontalChanged) {
            layout = null
            layouts.clear()
        }
        invalidate()
    }

    fun getGravity(): Int = gravity

    fun setMaxLines(lines: Int) {
        if (lines == maxLines) return
        maxLines = lines
        textChanged()
    }

    fun getMaxLines(): Int = maxLines

    fun setEllipsize(where: TextUtils.TruncateAt?) {
        if (where == ellipsize) return
        ellipsize = where
        textChanged()
    }

    fun setShadowLayer(radius: Float, dx: Float, dy: Float, color: Int) {
        paint.setShadowLayer(radius, dx, dy, color)
        shadowRadius = radius
        shadowDx = dx
        shadowDy = dy
        invalidate()
    }

    /** The laid-out paragraph as drawn, or null before the first measure. */
    fun getLayout(): Layout? = layout

    // --- Layout ------------------------------------------------------------

    private fun textChanged() {
        layout = null
        layouts.clear()
        desiredWidth = -1
        requestLayout()
        invalidate()
    }

    private fun isRtl(): Boolean =
        TextDirectionHeuristics.FIRSTSTRONG_LTR.isRtl(text, 0, text.length)

    /** `TextView.getLayoutAlignment` for this gravity, with LEFT/RIGHT resolved
     *  against the paragraph direction (their `Layout.Alignment`s are hidden). */
    private fun alignment(): Layout.Alignment =
        when (gravity and Gravity.RELATIVE_HORIZONTAL_GRAVITY_MASK) {
            Gravity.START -> Layout.Alignment.ALIGN_NORMAL
            Gravity.END -> Layout.Alignment.ALIGN_OPPOSITE
            Gravity.LEFT -> if (isRtl()) Layout.Alignment.ALIGN_OPPOSITE else Layout.Alignment.ALIGN_NORMAL
            Gravity.RIGHT -> if (isRtl()) Layout.Alignment.ALIGN_NORMAL else Layout.Alignment.ALIGN_OPPOSITE
            Gravity.CENTER_HORIZONTAL -> Layout.Alignment.ALIGN_CENTER
            else -> Layout.Alignment.ALIGN_NORMAL
        }

    /** `TextView`'s boring check: on API 33+ with fallback line spacing (a
     *  fallback font can raise a single line too); below, the two-argument
     *  form, which is first-strong LTR. */
    private fun boring(): BoringLayout.Metrics? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            BoringLayout.isBoring(text, paint, TextDirectionHeuristics.FIRSTSTRONG_LTR, defaults.fallbackLineSpacing, null)
        } else {
            BoringLayout.isBoring(text, paint)
        }

    private fun boringLayout(
        w: Int,
        align: Layout.Alignment,
        b: BoringLayout.Metrics,
        ellipsizeAt: TextUtils.TruncateAt?,
    ): BoringLayout =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            BoringLayout.make(text, paint, w, align, b, false, ellipsizeAt, w, defaults.fallbackLineSpacing)
        } else if (ellipsizeAt != null) {
            BoringLayout.make(text, paint, w, align, 1f, 0f, b, false, ellipsizeAt, w)
        } else {
            BoringLayout.make(text, paint, w, align, 1f, 0f, b, false)
        }

    /** The unwrapped width of the text, as `TextView.onMeasure` computes it. */
    private fun desiredWidth(): Int {
        if (desiredWidth < 0) {
            val b = boring()
            desiredWidth = b?.width ?: ceil(Layout.getDesiredWidth(text, paint)).toInt()
        }
        return desiredWidth
    }

    /** The layout at content width [width], built once per width. */
    private fun layoutFor(width: Int): Layout {
        val w = maxOf(width, 0)
        for (i in layouts.indices) {
            if (layouts[i].first == w) {
                layout = layouts[i].second
                return layouts[i].second
            }
        }
        val b = boring()
        val align = alignment()
        // TextView keeps a boring (single-direction, single-line) paragraph
        // that fits, or that it ellipsizes onto one line, in a BoringLayout.
        val l = if (b != null && b.width <= w && maxLines >= 1) {
            boringLayout(w, align, b, null)
        } else if (b != null && maxLines == 1 && ellipsize != null) {
            boringLayout(w, align, b, ellipsize)
        } else {
            val builder = StaticLayout.Builder.obtain(text, 0, text.length, paint, w)
                .setAlignment(align)
                .setTextDirection(TextDirectionHeuristics.FIRSTSTRONG_LTR)
                .setIncludePad(false)
                .setLineSpacing(0f, 1f)
                .setBreakStrategy(defaults.breakStrategy)
                .setHyphenationFrequency(defaults.hyphenationFrequency)
                .setJustificationMode(defaults.justificationMode)
                .setMaxLines(maxLines)
            if (ellipsize != null) builder.setEllipsize(ellipsize).setEllipsizedWidth(w)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                builder.setUseLineSpacingFromFallbacks(defaults.fallbackLineSpacing)
            }
            builder.build()
        }
        if (layouts.size == LAYOUT_CACHE) layouts.removeAt(0)
        layouts.add(w to l)
        layout = l
        return l
    }

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        val size = measureSize(widthMeasureSpec, heightMeasureSpec)
        setMeasuredDimension((size ushr 32).toInt(), size.toInt())
    }

    /**
     * The size [onMeasure] gives for these specs, packed `width << 32 |
     * height`, without measuring the view: the layout engine's probe. It
     * changes no view state, so unlike `View.measure` on a `TextView` there
     * is nothing to put back afterwards, and it skips `View.measure`'s own
     * cache and bookkeeping, which cost more than the answer.
     */
    fun measureSize(widthMeasureSpec: Int, heightMeasureSpec: Int): Long {
        val wMode = MeasureSpec.getMode(widthMeasureSpec)
        val wSize = MeasureSpec.getSize(widthMeasureSpec)
        val hMode = MeasureSpec.getMode(heightMeasureSpec)
        val hSize = MeasureSpec.getSize(heightMeasureSpec)
        val padH = paddingLeft + paddingRight
        val padV = paddingTop + paddingBottom
        var width = if (wMode == MeasureSpec.EXACTLY) {
            wSize
        } else {
            val w = maxOf(desiredWidth() + padH, suggestedMinimumWidth)
            if (wMode == MeasureSpec.AT_MOST) minOf(w, wSize) else w
        }
        width = maxOf(width, 0)
        val l = layoutFor(width - padH)
        // `TextView.getDesiredHeight`: a line limit without ellipsis keeps
        // every line in the layout (StaticLayout only drops lines it
        // ellipsizes) and caps the height at the limit; the rest is clipped.
        val lines = minOf(l.lineCount, maxLines)
        var height = maxOf(l.getLineTop(lines) + padV, suggestedMinimumHeight)
        if (hMode == MeasureSpec.EXACTLY) height = hSize
        else if (hMode == MeasureSpec.AT_MOST) height = minOf(height, hSize)
        return (width.toLong() shl 32) or (height.toLong() and 0xffffffffL)
    }

    /** `TextView.getVerticalOffset`: where the paragraph sits in a taller box. */
    private fun verticalOffset(l: Layout): Int {
        val box = height - paddingTop - paddingBottom
        val textHeight = l.height
        if (textHeight >= box) return 0
        return when (gravity and Gravity.VERTICAL_GRAVITY_MASK) {
            Gravity.CENTER_VERTICAL -> (box - textHeight) shr 1
            Gravity.BOTTOM -> box - textHeight
            else -> 0
        }
    }

    override fun onDraw(canvas: Canvas) {
        val l = layoutFor(width - paddingLeft - paddingRight)
        canvas.save()
        // TextView clips to the padding box, grown by any shadow.
        val r = shadowRadius
        canvas.clipRect(
            paddingLeft + minOf(0f, shadowDx - r),
            paddingTop + minOf(0f, shadowDy - r),
            width - paddingRight + maxOf(0f, shadowDx + r),
            height - paddingBottom + maxOf(0f, shadowDy + r),
        )
        canvas.translate(paddingLeft.toFloat(), (paddingTop + verticalOffset(l)).toFloat())
        l.draw(canvas)
        canvas.restore()
    }

    // --- Accessibility -----------------------------------------------------

    override fun getAccessibilityClassName(): CharSequence = TextView::class.java.name

    override fun onInitializeAccessibilityNodeInfo(info: AccessibilityNodeInfo) {
        super.onInitializeAccessibilityNodeInfo(info)
        info.text = text
    }

    override fun onPopulateAccessibilityEvent(event: AccessibilityEvent) {
        super.onPopulateAccessibilityEvent(event)
        if (text.isNotEmpty()) event.text.add(text)
    }

    private companion object {
        const val LAYOUT_CACHE = 3
    }

    /**
     * What a label inherits from the theme, read once from a real
     * `TextView` for each theme and font scale. Copying them is what makes
     * an unstyled label match the `TextView` it replaces.
     */
    private class Defaults(
        val paint: TextPaint,
        val color: Int,
        val breakStrategy: Int,
        val hyphenationFrequency: Int,
        val justificationMode: Int,
        val fallbackLineSpacing: Boolean,
        val fontScale: Float,
    ) {
        companion object {
            private val byTheme = WeakHashMap<Any, Defaults>()

            fun of(context: Context): Defaults {
                val key: Any = context.theme ?: context
                val scale = context.resources.configuration.fontScale
                byTheme[key]?.let { if (it.fontScale == scale) return it }
                val tv = TextView(context)
                val d = Defaults(
                    paint = TextPaint().apply { set(tv.paint) },
                    color = tv.currentTextColor,
                    breakStrategy = tv.breakStrategy,
                    hyphenationFrequency = tv.hyphenationFrequency,
                    justificationMode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) tv.justificationMode else 0,
                    fallbackLineSpacing = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) tv.isFallbackLineSpacing else false,
                    fontScale = scale,
                )
                byTheme[key] = d
                return d
            }
        }
    }
}
