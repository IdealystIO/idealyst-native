package io.idealyst.runtime

import android.view.Choreographer

/**
 * `Choreographer.FrameCallback` whose `doFrame` forwards into Rust via
 * the scheduler's thread-local callback registry (keyed by a long id,
 * NOT a raw pointer — see `RustScheduledRunnable` for why we abandoned
 * the leaked-pointer design).
 *
 * # Why a frame callback and not a `Handler.postDelayed`
 *
 * The Rust scheduler uses this to run the Taffy layout pass for a
 * dynamically-mounted subtree (the modal/portal content mounted when an
 * open signal flips). A `Choreographer` frame callback runs at the
 * START of the next frame, in the animation/input callback phase,
 * BEFORE that frame's measure/layout/draw traversal. Applying Taffy
 * frames here lands the geometry on the views before they are drawn, so
 * the content never paints unlaid-out (card at the origin) for one
 * frame. A `Handler.postDelayed(0/16ms)` posts a message that typically
 * runs AFTER the frame's traversal, so the unlaid-out content draws
 * once and visibly snaps into place — the bug this class prevents.
 *
 * One-shot: `doFrame` consumes the registered closure via `nativeInvoke`
 * exactly like [`RustScheduledRunnable`]. Choreographer frame callbacks
 * are inherently single-fire (you re-post to get another), so there is
 * no `removeCallbacks` cancellation path wired here — the only caller
 * (the coalesced layout pass) never cancels.
 */
class RustFrameCallback(private val nativePtr: Long) : Choreographer.FrameCallback {
    override fun doFrame(frameTimeNanos: Long) {
        nativeInvoke(nativePtr)
    }

    fun post() {
        Choreographer.getInstance().postFrameCallback(this)
    }

    private external fun nativeInvoke(ptr: Long)
}
