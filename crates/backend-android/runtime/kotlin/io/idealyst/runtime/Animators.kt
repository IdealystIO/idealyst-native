package io.idealyst.runtime

import android.animation.ArgbEvaluator
import android.animation.ValueAnimator
import android.graphics.drawable.GradientDrawable
import android.view.View
import android.view.animation.Interpolator

/**
 * Helpers used by the Rust backend for animations that don't map
 * cleanly to `ObjectAnimator`'s reflection-based property finder.
 *
 * Each `animate*` returns the `ValueAnimator` so the Rust side can
 * store a handle in its per-node animator cache and `cancel()` it
 * before launching a replacement on the next value change.
 *
 * # Threading
 *
 * Everything here runs on the Android UI thread (the same thread the
 * framework's render walker runs on). `ValueAnimator` posts its
 * frame callbacks to the Choreographer of the calling thread, so
 * keeping all the setup on the UI thread is required.
 */
object Animators {
    /**
     * Animate one side of a View's padding from `from` (px) to `to`
     * (px), preserving the other three sides. `side` encodes which
     * edge: 0=left, 1=top, 2=right, 3=bottom.
     *
     * We use a `ValueAnimator.ofInt(from, to)` with an update listener
     * that re-invokes `setPadding(l, t, r, b)` reading the live values
     * from the View — so a concurrent animator on a different side
     * won't be overwritten by this one.
     */
    @JvmStatic
    fun animatePaddingSide(
        view: View,
        side: Int,
        from: Int,
        to: Int,
        durationMs: Long,
        interpolator: Interpolator
    ): ValueAnimator {
        val anim = ValueAnimator.ofInt(from, to)
        anim.duration = durationMs
        anim.interpolator = interpolator
        anim.addUpdateListener { a ->
            val v = a.animatedValue as Int
            val l = if (side == 0) v else view.paddingLeft
            val t = if (side == 1) v else view.paddingTop
            val r = if (side == 2) v else view.paddingRight
            val b = if (side == 3) v else view.paddingBottom
            view.setPadding(l, t, r, b)
        }
        anim.start()
        return anim
    }

    /**
     * Animate `GradientDrawable.setStroke(width, color)` from (fromW,
     * fromC) to (toW, toC). The width is interpolated linearly along
     * the animation fraction; the color uses `ArgbEvaluator`.
     *
     * Stroke is a combined setter (width + color in one call), so we
     * compute both intermediate values on each tick and reissue
     * `setStroke`. One ValueAnimator drives both axes.
     */
    @JvmStatic
    fun animateStroke(
        drawable: GradientDrawable,
        fromW: Int,
        toW: Int,
        fromC: Int,
        toC: Int,
        durationMs: Long,
        interpolator: Interpolator
    ): ValueAnimator {
        val anim = ValueAnimator.ofFloat(0f, 1f)
        anim.duration = durationMs
        anim.interpolator = interpolator
        val argb = ArgbEvaluator()
        anim.addUpdateListener { a ->
            val f = a.animatedFraction
            val w = (fromW + (toW - fromW) * f).toInt()
            val c = argb.evaluate(f, fromC, toC) as Int
            drawable.setStroke(w, c)
        }
        anim.start()
        return anim
    }

    /**
     * Animate the four corner radii of a `GradientDrawable`
     * independently. `from` and `to` are length-4 arrays in the order
     * [tl, tr, br, bl] (px). On each tick we build the 8-element
     * float array `setCornerRadii` expects (x- and y-radius repeated
     * per corner) from the interpolated values.
     */
    @JvmStatic
    fun animateCornerRadii(
        drawable: GradientDrawable,
        from: FloatArray,
        to: FloatArray,
        durationMs: Long,
        interpolator: Interpolator
    ): ValueAnimator {
        require(from.size == 4 && to.size == 4) { "from/to must have 4 elements (tl, tr, br, bl)" }
        val anim = ValueAnimator.ofFloat(0f, 1f)
        anim.duration = durationMs
        anim.interpolator = interpolator
        anim.addUpdateListener { a ->
            val f = a.animatedFraction
            val tl = from[0] + (to[0] - from[0]) * f
            val tr = from[1] + (to[1] - from[1]) * f
            val br = from[2] + (to[2] - from[2]) * f
            val bl = from[3] + (to[3] - from[3]) * f
            // setCornerRadii expects [tl, tl, tr, tr, br, br, bl, bl]
            // (x-radius, y-radius per corner).
            drawable.cornerRadii = floatArrayOf(tl, tl, tr, tr, br, br, bl, bl)
        }
        anim.start()
        return anim
    }
}
