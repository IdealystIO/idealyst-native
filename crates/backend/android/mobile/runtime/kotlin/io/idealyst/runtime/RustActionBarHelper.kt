package io.idealyst.runtime

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.ColorFilter
import android.graphics.Paint
import android.graphics.PixelFormat
import android.graphics.drawable.Drawable
import android.view.View
import android.widget.Toolbar

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
