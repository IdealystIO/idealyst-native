package io.idealyst.runtime

import android.content.Context
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import android.widget.TextView

/**
 * The backend's most-created views, built and configured in one JNI call.
 *
 * A mounted list creates a view and a label per cell. Built from Rust, a
 * label was about ten JNI calls (a class lookup, the constructor, each
 * setter looked up by name), which cost more than the views themselves.
 */
object RustViewFactory {
    /** A `view`: a `FrameLayout` the layout pass positions by its margins. */
    @JvmStatic
    fun view(context: Context): FrameLayout {
        val v = FrameLayout(context)
        v.layoutParams = defaultLayoutParams()
        return v
    }

    /** A `text`: a `TextView` holding `content`. */
    @JvmStatic
    fun text(context: Context, content: String): TextView {
        val v = TextView(context)
        v.text = content
        // Strip the legacy `fontPadding` insets Android adds above the
        // ascent and below the descent. iOS (UILabel) measures text
        // without these extras, and the style system gives authors
        // explicit padding / line_height controls, so matching iOS's
        // layout means turning Android's off: without this, tight text
        // rows (sidebar nav links) look taller on Android.
        v.includeFontPadding = false
        // Pin line spacing to 1.0× (no extra) so multi-line text and
        // single-line rows match UILabel's metrics. Android's default
        // multiplier varies by API level.
        v.setLineSpacing(0f, 1f)
        // Strip the theme's default minimum height (~48dp on some API
        // levels, for touch targets): the measure_fn must report the true
        // text-metric height so flex siblings pack tightly.
        v.minHeight = 0
        v.minimumHeight = 0
        // UILabel centres single-line text vertically in its frame; a
        // TextView defaults to top|start, which leaves the glyphs flush
        // against the frame's top edge (visible in any styled control
        // with vertical padding). Multi-line text fills its frame, so
        // this only shows for single-line labels in a taller frame.
        v.gravity = Gravity.CENTER_VERTICAL or Gravity.START
        // TextView's constructor raises its importance for accessibility
        // to YES. The backend's accessibility mapping (`a11y_policy`)
        // decides importance itself, and AUTO is its value for an
        // unlabelled label, so put the View default back.
        v.importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_AUTO
        v.layoutParams = defaultLayoutParams()
        return v
    }

    /**
     * `MarginLayoutParams`, not the bare `ViewGroup.LayoutParams`: a
     * margin-aware parent (`ScrollView.measureChildWithMargins`) casts its
     * child's params and throws `ClassCastException` otherwise.
     */
    private fun defaultLayoutParams() =
        ViewGroup.MarginLayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.WRAP_CONTENT,
        )
}
