package io.idealyst.runtime

import android.animation.Animator
import android.animation.AnimatorListenerAdapter
import android.animation.AnimatorSet
import android.animation.ObjectAnimator
import android.view.View
import android.view.ViewGroup
import android.view.animation.PathInterpolator
import android.widget.FrameLayout
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback

/**
 * Native stack presenter for the outlet-model stack navigator — the
 * Android side of `android-navigator-helpers::presenter`.
 *
 * The Rust handler still owns everything logical (mounting, retention,
 * the back stack). This owns only placement and transition: a container
 * seated in the navigator outlet, one full-size wrapper per stack
 * screen, the push/pop/replace transition, and system Back.
 *
 * # Why a wrapper per screen
 *
 * The transition animates the wrapper's translationX/alpha, never the
 * screen view's own: the screen root is author content whose transform
 * and opacity belong to its style. The wrapper is MATCH_PARENT in the
 * container and the screen is laid out against the outlet's size (a
 * hosted root, `backend_android::host_screen_root`), so the two match.
 *
 * # The transition
 *
 * Material's shared-axis X, the pattern Material specifies for forward
 * and backward navigation between screens in a hierarchy: both screens
 * slide [SLIDE_DP] along the axis while the outgoing one fades out over
 * the first [FADE_OUT_FRACTION] of the duration and the incoming one fades
 * in over the rest. Pop runs it mirrored. `ValueAnimator` honours the
 * system animator duration scale, so "Remove animations" ends it at once.
 *
 * # System Back
 *
 * [backCallback] is enabled while more than one screen is stacked. Back
 * pops natively and then reports through [nativeUserPop]; the handler
 * reconciles its logical stack WITHOUT calling [pop] again (that would
 * pop twice). The dispatcher is LIFO, so a stack mounted inside a pushed
 * screen registers later and takes Back first, which is the nesting
 * order a user expects.
 */
class RustStackPresenter(private val outlet: ViewGroup) {
    /** The leaked Rust state [nativeUserPop] reaches. Set by Rust right
     *  after construction; 0 until then, and [dispose] runs before Rust
     *  frees it. */
    @JvmField
    var nativePtr: Long = 0

    private val context = outlet.context
    private val density = context.resources.displayMetrics.density

    /** Seated in the outlet; holds the wrappers, top-of-stack last. */
    val container: FrameLayout = FrameLayout(context).apply {
        layoutParams = FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT,
        )
    }

    private val hosts = mutableListOf<FrameLayout>()

    /** The transition in flight, ended (jumped to its final state, end
     *  listeners run) before any new operation so two never overlap. */
    private var running: AnimatorSet? = null

    private val backCallback = object : OnBackPressedCallback(false) {
        override fun handleOnBackPressed() {
            if (hosts.size <= 1) return
            pop()
            nativeUserPop(nativePtr)
        }
    }

    init {
        outlet.addView(container)
        (context as? ComponentActivity)?.onBackPressedDispatcher?.addCallback(backCallback)
    }

    /** The bottom screen arriving at mount, unanimated. */
    fun seat(view: View) = reset(view)

    /** Collapse to [view] alone, unanimated. */
    fun reset(view: View) {
        finishRunning()
        for (h in hosts) container.removeView(h)
        hosts.clear()
        hosts.add(wrap(view))
        syncBack()
    }

    fun push(view: View) {
        finishRunning()
        val outgoing = hosts.lastOrNull()
        val incoming = wrap(view)
        hosts.add(incoming)
        syncBack()
        transition(outgoing, incoming, forward = true) {
            outgoing?.visibility = View.GONE
        }
    }

    /** Pop the top, revealing the screen below. No-op at the root. */
    fun pop() {
        finishRunning()
        if (hosts.size <= 1) return
        val outgoing = hosts.removeAt(hosts.size - 1)
        val incoming = hosts.last()
        incoming.visibility = View.VISIBLE
        syncBack()
        transition(outgoing, incoming, forward = false) {
            container.removeView(outgoing)
        }
    }

    /** Swap the top for [view], keeping the stack below. Animated like a
     *  push: on Android as on iOS, replacing the top is a navigation. */
    fun replace(view: View) {
        finishRunning()
        val outgoing = if (hosts.isEmpty()) null else hosts.removeAt(hosts.size - 1)
        val incoming = wrap(view)
        hosts.add(incoming)
        syncBack()
        transition(outgoing, incoming, forward = true) {
            outgoing?.let { container.removeView(it) }
        }
    }

    /** Release the Back callback and take the container out of the
     *  outlet. Called when the navigator is torn down; without it the
     *  dispatcher would keep this presenter alive and keep handling Back. */
    fun dispose() {
        finishRunning()
        backCallback.remove()
        outlet.removeView(container)
    }

    private fun wrap(view: View): FrameLayout {
        (view.parent as? ViewGroup)?.removeView(view)
        val host = FrameLayout(context).apply {
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT,
            )
            addView(view)
        }
        container.addView(host)
        return host
    }

    private fun syncBack() {
        backCallback.isEnabled = hosts.size > 1
    }

    private fun finishRunning() {
        running?.end()
        running = null
    }

    private fun transition(
        outgoing: View?,
        incoming: View,
        forward: Boolean,
        onEnd: () -> Unit,
    ) {
        val slide = SLIDE_DP * density * (if (forward) 1f else -1f)
        val fadeOutMs = (DURATION_MS * FADE_OUT_FRACTION).toLong()
        val animators = mutableListOf<Animator>(
            ObjectAnimator.ofFloat(incoming, View.TRANSLATION_X, slide, 0f).setDuration(DURATION_MS),
            ObjectAnimator.ofFloat(incoming, View.ALPHA, 0f, 1f).apply {
                startDelay = fadeOutMs
                duration = DURATION_MS - fadeOutMs
            },
        )
        if (outgoing != null) {
            animators += ObjectAnimator.ofFloat(outgoing, View.TRANSLATION_X, 0f, -slide).setDuration(DURATION_MS)
            animators += ObjectAnimator.ofFloat(outgoing, View.ALPHA, 1f, 0f).setDuration(fadeOutMs)
        }
        val set = AnimatorSet().apply {
            playTogether(animators)
            interpolator = EMPHASIZED
            addListener(object : AnimatorListenerAdapter() {
                override fun onAnimationEnd(animation: Animator) {
                    // Reset what the transition touched so a covered
                    // screen comes back at rest when it is revealed.
                    for (v in listOfNotNull(outgoing, incoming)) {
                        v.translationX = 0f
                        v.alpha = 1f
                    }
                    onEnd()
                    if (running === animation) running = null
                }
            })
        }
        running = set
        set.start()
    }

    private external fun nativeUserPop(ptr: Long)

    companion object {
        /** Material shared-axis travel distance. */
        private const val SLIDE_DP = 30f
        /** Material "medium" transition duration for full-screen navigation. */
        private const val DURATION_MS = 300L
        /** Shared-axis fade-through: the outgoing screen is gone by 35%. */
        private const val FADE_OUT_FRACTION = 0.35f
        /** Material standard easing, `cubic-bezier(0.2, 0, 0, 1)`. */
        private val EMPHASIZED = PathInterpolator(0.2f, 0f, 0f, 1f)
    }
}
