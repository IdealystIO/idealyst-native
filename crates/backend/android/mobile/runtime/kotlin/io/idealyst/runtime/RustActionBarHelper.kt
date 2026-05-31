package io.idealyst.runtime

import android.animation.ArgbEvaluator
import android.animation.ObjectAnimator
import android.animation.ValueAnimator
import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.ColorFilter
import android.graphics.Paint
import android.graphics.PixelFormat
import android.graphics.drawable.ColorDrawable
import android.graphics.drawable.Drawable
import android.view.View
import android.widget.Toolbar
import android.widget.TextView

/**
 * Programmatic 3-line "hamburger" drawable. Used as the in-tree
 * Toolbar's `navigationIcon` when a drawer navigator screen has a
 * `header_left` button.
 *
 * Programmatic (not a vector drawable resource) so the run-android
 * pipeline doesn't have to compile / pack any res/drawable XML —
 * everything ships in the runtime Kotlin module.
 */
class HamburgerDrawable : Drawable() {
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.BLACK
        style = Paint.Style.STROKE
        strokeCap = Paint.Cap.ROUND
    }

    /** Current stroke color, exposed so theme-fade animators have a
     *  `from` value without poking at the private Paint. */
    val strokeColor: Int get() = paint.color

    fun setStrokeColor(c: Int) {
        paint.color = c
        invalidateSelf()
    }

    override fun draw(canvas: Canvas) {
        val w = bounds.width().toFloat()
        val h = bounds.height().toFloat()
        if (w <= 0f || h <= 0f) return
        paint.strokeWidth = h * 0.10f
        val x0 = bounds.left + w * 0.18f
        val x1 = bounds.right - w * 0.18f
        val cy = bounds.top + h / 2f
        val gap = h * 0.22f
        canvas.drawLine(x0, cy - gap, x1, cy - gap, paint)
        canvas.drawLine(x0, cy, x1, cy, paint)
        canvas.drawLine(x0, cy + gap, x1, cy + gap, paint)
    }

    override fun setAlpha(alpha: Int) {
        paint.alpha = alpha
    }

    override fun setColorFilter(cf: ColorFilter?) {
        paint.colorFilter = cf
    }

    @Suppress("DEPRECATION")
    override fun getOpacity(): Int = PixelFormat.TRANSLUCENT

    override fun getIntrinsicWidth() = 96
    override fun getIntrinsicHeight() = 96
}

/**
 * Builds and configures an in-tree `android.widget.Toolbar` for a
 * drawer-navigator screen.
 *
 * Why in-tree (not the system ActionBar): the drawer panel needs to
 * cover the toolbar when it slides in. The system ActionBar lives in
 * the window decor *above* the activity's `setContentView`, so a
 * `DrawerLayout` placed inside content can never overlay it. An
 * in-tree Toolbar sits inside the DrawerLayout's body container,
 * which puts the drawer panel z-above it for free.
 *
 * The activity's theme must be `*.NoActionBar` to avoid a double bar;
 * `AndroidManifest{,Aas}.xml` templates set that.
 *
 * Click wiring: `header_left.on_press` is delivered to the Rust side
 * by a `RustClickListener` whose pointer is the leaked
 * `HeaderButtonCallback`. The same JNI export
 * (`Java_io_idealyst_runtime_RustActionBarHelper_nativeInvoke`) the
 * old system-ActionBar path used dispatches it — the only difference
 * now is who invokes it (the Toolbar's `OnClickListener` vs. the
 * Activity's `onOptionsItemSelected`).
 */
object RustActionBarHelper {
    /**
     * Build a Toolbar pre-configured for a drawer screen.
     *
     * - `title`: shown on the bar; null leaves it blank.
     * - `leftCallbackPtr`: pointer to a leaked `HeaderButtonCallback`.
     *   `0` ⇒ no left button (no hamburger).
     * - `bgColorCss` / `titleColorCss` / `tintColorCss`: CSS color
     *   strings (`"#RRGGBB"`, `"#AARRGGBB"`, `"rgb(...)"`, etc.) or
     *   null to keep the default. `tintColorCss` colors both the
     *   navigation icon (hamburger) and any future action items.
     */
    @JvmStatic
    fun buildToolbar(
        context: Context,
        title: String?,
        leftCallbackPtr: Long,
        bgColorCss: String?,
        titleColorCss: String?,
        tintColorCss: String?,
    ): Toolbar {
        val bar = Toolbar(context)
        if (title != null) {
            bar.title = title
        }
        bar.setTitleTextColor(parseColorOr(titleColorCss, Color.BLACK))
        bar.setBackgroundColor(parseColorOr(bgColorCss, Color.WHITE))
        if (leftCallbackPtr != 0L) {
            val drawable = HamburgerDrawable()
            val tint = parseColorOrNull(tintColorCss)
            if (tint != null) {
                drawable.setStrokeColor(tint)
            }
            bar.navigationIcon = drawable
            bar.setNavigationOnClickListener(object : View.OnClickListener {
                override fun onClick(v: View) {
                    nativeInvoke(leftCallbackPtr)
                }
            })
        }
        return bar
    }

    /// Parse a CSS-ish color string. Returns `fallback` for null or
    /// unparseable input. `Color.parseColor` recognizes `#RGB`,
    /// `#RRGGBB`, `#AARRGGBB`, and the named colors — but throws
    /// IllegalArgumentException on anything else (notably `rgb(...)`
    /// and `rgba(...)`), so wrap with a manual `rgb(...)` decoder
    /// before delegating.
    private fun parseColorOr(css: String?, fallback: Int): Int =
        parseColorOrNull(css) ?: fallback

    private fun parseColorOrNull(css: String?): Int? {
        val s = css?.trim() ?: return null
        if (s.isEmpty()) return null
        // Fast paths for the two `rgb(...)` shapes the framework
        // emits. Color.parseColor doesn't support them, so we
        // hand-roll a tiny parser rather than depend on a regex.
        if (s.startsWith("rgba(") || s.startsWith("rgb(")) {
            val open = s.indexOf('(')
            val close = s.indexOf(')')
            if (open < 0 || close <= open) return null
            val parts = s.substring(open + 1, close).split(',')
            if (parts.size != 3 && parts.size != 4) return null
            val r = parts[0].trim().toIntOrNull() ?: return null
            val g = parts[1].trim().toIntOrNull() ?: return null
            val b = parts[2].trim().toIntOrNull() ?: return null
            val a = if (parts.size == 4) {
                val raw = parts[3].trim().toFloatOrNull() ?: return null
                (raw.coerceIn(0f, 1f) * 255f).toInt()
            } else 255
            return Color.argb(a, r.coerceIn(0, 255), g.coerceIn(0, 255), b.coerceIn(0, 255))
        }
        return try {
            Color.parseColor(s)
        } catch (e: IllegalArgumentException) {
            null
        }
    }

    /** Cross-fade duration for theme-driven Toolbar / body color swaps.
     *  Matches the framework's 250 ms body-background transition so
     *  the bar and the page beneath dissolve in lockstep. */
    private const val THEME_FADE_MS = 250L

    /** Pull the current solid color out of a View's background, or
     *  fall back to `default` if the background isn't a plain
     *  `ColorDrawable`. Needed because ObjectAnimator on
     *  "backgroundColor" reads via getBackground+ColorDrawable.color;
     *  if there's no prior color we still need a starting point for
     *  the ArgbEvaluator. */
    private fun currentSolidBackgroundColor(view: View, default: Int): Int {
        val bg = view.background
        return if (bg is ColorDrawable) bg.color else default
    }

    /**
     * Re-tint the Toolbar's background. Called by the navigator's
     * reactive `header_style` Effect on theme change — the bar itself
     * isn't rebuilt; just the background color gets swapped.
     */
    @JvmStatic
    fun setToolbarBackground(bar: Toolbar, css: String?) {
        val to = parseColorOrNull(css) ?: return
        val from = currentSolidBackgroundColor(bar, to)
        if (from == to) {
            bar.setBackgroundColor(to)
            return
        }
        val anim = ValueAnimator.ofObject(ArgbEvaluator(), from, to).apply {
            duration = THEME_FADE_MS
            addUpdateListener { a -> bar.setBackgroundColor(a.animatedValue as Int) }
        }
        anim.start()
    }

    /**
     * Re-tint the Toolbar's title text. Same lifecycle as
     * [setToolbarBackground] — driven by the navigator's
     * `title_style` Effect.
     */
    @JvmStatic
    fun setToolbarTitleColor(bar: Toolbar, css: String?) {
        val to = parseColorOrNull(css) ?: return
        // Toolbar stores its title in an internally-managed TextView
        // (created on first `setTitle`). Walk the bar's children to
        // find it; if absent, just set the color statically (next
        // setTitle will pick it up).
        var titleView: TextView? = null
        for (i in 0 until bar.childCount) {
            val c = bar.getChildAt(i)
            if (c is TextView) {
                titleView = c
                break
            }
        }
        if (titleView == null) {
            bar.setTitleTextColor(to)
            return
        }
        val from = titleView.currentTextColor
        if (from == to) {
            bar.setTitleTextColor(to)
            return
        }
        val tv = titleView
        val anim = ValueAnimator.ofObject(ArgbEvaluator(), from, to).apply {
            duration = THEME_FADE_MS
            addUpdateListener { a ->
                val v = a.animatedValue as Int
                tv.setTextColor(v)
                // Keep Toolbar's "future title-set" color in sync so
                // a title change mid-fade picks up the live color.
                bar.setTitleTextColor(v)
            }
        }
        anim.start()
    }

    /**
     * Paint an arbitrary View's background from a CSS color string.
     * Used by the navigator's `apply_navigator_body_style` hook to
     * fill the body container reactively on theme change. Distinct
     * from `setToolbarBackground` only in argument type — kept
     * separate so the JNI signatures match Toolbar / View precisely
     * and we don't have to upcast at call sites.
     */
    @JvmStatic
    fun setViewBackground(view: View, css: String?) {
        val to = parseColorOrNull(css) ?: return
        val from = currentSolidBackgroundColor(view, to)
        if (from == to) {
            view.setBackgroundColor(to)
            return
        }
        val anim = ValueAnimator.ofObject(ArgbEvaluator(), from, to).apply {
            duration = THEME_FADE_MS
            addUpdateListener { a -> view.setBackgroundColor(a.animatedValue as Int) }
        }
        anim.start()
    }

    /**
     * Re-tint the Toolbar's navigation icon (the hamburger / back
     * chevron). Same lifecycle as the others.
     */
    @JvmStatic
    fun setToolbarNavIconTint(bar: Toolbar, css: String?) {
        val to = parseColorOrNull(css) ?: return
        val drawable = bar.navigationIcon ?: return
        if (drawable is HamburgerDrawable) {
            val from = drawable.strokeColor
            if (from == to) {
                drawable.setStrokeColor(to)
                return
            }
            val anim = ValueAnimator.ofObject(ArgbEvaluator(), from, to).apply {
                duration = THEME_FADE_MS
                addUpdateListener { a -> drawable.setStrokeColor(a.animatedValue as Int) }
            }
            anim.start()
        } else {
            // Generic fallback for future non-hamburger icons. `setTint`
            // is instant; an icon-class-specific animator would go here
            // when we ship a non-Hamburger icon.
            drawable.setTint(to)
        }
    }

    /**
     * Legacy dispatch path retained for any caller that still routes
     * the home button through the Activity's `onOptionsItemSelected`.
     * In the new in-tree-Toolbar architecture this is never called —
     * left here so `MainActivity{,Aas}.java`'s override compiles
     * without changes. Returns `false` unconditionally so the
     * Activity's super.onOptionsItemSelected runs.
     */
    @JvmStatic
    fun dispatchHomePress(): Boolean = false

    /// Jumps into Rust to invoke the boxed `Rc<dyn Fn()>` at `ptr`.
    /// Must match the JNI export signature declared in
    /// `crates/backend/android/mobile/src/imp/jni_exports.rs`.
    @JvmStatic
    external fun nativeInvoke(ptr: Long)
}
