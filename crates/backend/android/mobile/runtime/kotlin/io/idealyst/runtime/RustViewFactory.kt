package io.idealyst.runtime

import android.content.Context
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import android.widget.TextView

/**
 * The backend's most-created views, built and configured in one JNI call:
 * a mounted list creates a view and a label per cell.
 */
object RustViewFactory {
    /** A `view`: a `FrameLayout` the layout pass positions by its margins. */
    @JvmStatic
    fun view(context: Context): FrameLayout {
        val v = FrameLayout(context)
        v.layoutParams = defaultLayoutParams()
        return v
    }

    /** A `text`: a [RustLabel] holding `content`. Its defaults are the
     *  backend's TextView setup: see [RustLabel]. */
    @JvmStatic
    fun text(context: Context, content: String): RustLabel {
        val v = RustLabel(context)
        v.setText(content)
        v.layoutParams = defaultLayoutParams()
        return v
    }

    /**
     * A selectable `text` (`text(selectable = true)`): Android's own
     * selectable text, a `TextView` with `setTextIsSelectable(true)` —
     * long-press, handles, the copy menu — which a self-drawing
     * [RustLabel] cannot offer. Set up to render as a label does: no font
     * padding, line spacing 1.0, no minimum height, centred vertically at
     * the start, zero padding, and the View default importance for
     * accessibility (the constructor raises it).
     */
    @JvmStatic
    fun selectableText(context: Context, content: String): TextView {
        val v = TextView(context)
        v.includeFontPadding = false
        v.setLineSpacing(0f, 1f)
        v.minHeight = 0
        v.minimumHeight = 0
        v.gravity = Gravity.CENTER_VERTICAL or Gravity.START
        v.importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_AUTO
        v.setPadding(0, 0, 0, 0)
        v.setTextIsSelectable(true)
        v.text = content
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
