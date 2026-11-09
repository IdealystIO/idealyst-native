package io.idealyst.runtime

import android.view.View

/**
 * The layout engine's size probe of a view, in one JNI call.
 *
 * A Taffy measure_fn (a text leaf, an external view) asks a view for its
 * size several times per layout pass. Done from Rust, each probe was about
 * eight JNI crossings, each one looking its method up by name, which cost
 * more than the measure itself on a list of labels.
 */
object RustViewMeasure {
    /**
     * `view.measure(widthSpec, heightSpec)`, then put the view back at its
     * laid-out frame. Returns the measured size in px, packed as
     * `width << 32 | height`.
     *
     * The put-back: a probe is a real `View.measure`, so a `TextView`
     * rebuilds its text layout at the probe's width (a min-content probe
     * wraps a label to a word a line). Nothing guarantees a later measure
     * at the real frame: when the relayout leaves the frame unchanged (the
     * soft keyboard closing), Android's measure cache can answer
     * `EXACTLY(frame)` without calling `onMeasure`, and the label draws the
     * probe's layout in its full-size frame, cut to its first word. So
     * `forceLayout` bypasses the cache and a measure rebuilds the layout at
     * the frame the view is drawn in. A view not yet laid out has no frame
     * to restore, and its first layout measures it fresh.
     */
    @JvmStatic
    fun probe(view: View, widthSpec: Int, heightSpec: Int): Long {
        view.measure(widthSpec, heightSpec)
        val w = view.measuredWidth
        val h = view.measuredHeight
        val frameW = view.width
        val frameH = view.height
        if (frameW > 0 && frameH > 0) {
            view.forceLayout()
            view.measure(
                View.MeasureSpec.makeMeasureSpec(frameW, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(frameH, View.MeasureSpec.EXACTLY),
            )
        }
        return (w.toLong() shl 32) or (h.toLong() and 0xffffffffL)
    }
}
