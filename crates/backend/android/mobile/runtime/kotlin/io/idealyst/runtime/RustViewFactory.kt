package io.idealyst.runtime

import android.content.Context
import android.view.ViewGroup
import android.widget.FrameLayout

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
