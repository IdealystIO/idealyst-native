package io.idealyst.runtime

import android.view.MotionEvent
import android.view.View

/**
 * `View.OnTouchListener` that translates `MotionEvent`s into the
 * framework's raw `TouchEvent` shape and forwards each touch to
 * Rust via `nativeInvokeTouch`.
 *
 * One JNI call per (touch × phase change):
 * - `ACTION_DOWN` / `ACTION_POINTER_DOWN` → one Began for
 *   `actionIndex`.
 * - `ACTION_MOVE` → one Moved per active pointer.
 * - `ACTION_UP` / `ACTION_POINTER_UP` → one Ended for `actionIndex`.
 * - `ACTION_CANCEL` → one Cancelled per active pointer.
 *
 * The native trampoline returns a packed response:
 *   bit 0 = consumed
 *   bit 1 = claim
 *
 * If any touch in this MotionEvent was consumed, `onTouch` returns
 * `true` and Android won't bubble the event to ancestors. If any
 * touch had `claim`, we request that ancestor scroll containers
 * not intercept this gesture.
 *
 * See `docs/native-touch-backends-plan.md` for the design.
 */
class RustTouchListener(private val nativePtr: Long) : View.OnTouchListener {

    companion object {
        // Mirror `TouchPhase` in runtime-core.
        private const val PHASE_BEGAN = 0
        private const val PHASE_MOVED = 1
        private const val PHASE_ENDED = 2
        private const val PHASE_CANCELLED = 3

        private const val RESP_CONSUMED = 0x1
        private const val RESP_CLAIM = 0x2
    }

    private val screenLoc = IntArray(2)

    override fun onTouch(v: View, event: MotionEvent): Boolean {
        // Snapshot the view's screen position once per MotionEvent so
        // window-space coords can be computed without per-touch
        // `getLocationOnScreen` calls. The view doesn't move between
        // the dispatch calls for a single event.
        v.getLocationOnScreen(screenLoc)
        // MotionEvent coordinates are DEVICE PIXELS, but the framework's
        // coordinate space is logical dp everywhere else — Taffy layout frames,
        // the canvas Scene, and iOS's `locationInView` points. Reporting raw px
        // makes `TouchEvent::position` ~`density`× too large, so absolute-
        // position consumers (canvas drawing) land far off and gesture deltas
        // run `density`× too fast. Divide by density so Android matches iOS.
        val density = v.resources.displayMetrics.density.let { if (it > 0f) it else 1f }
        val screenX = screenLoc[0].toFloat() / density
        val screenY = screenLoc[1].toFloat() / density
        val timestampNs = event.eventTime * 1_000_000L

        var anyConsumed = false
        var anyClaim = false

        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> {
                val idx = event.actionIndex
                val resp = dispatch(event, idx, PHASE_BEGAN, screenX, screenY, density, timestampNs)
                if (resp and RESP_CONSUMED != 0) anyConsumed = true
                if (resp and RESP_CLAIM != 0) anyClaim = true
            }
            MotionEvent.ACTION_MOVE -> {
                for (i in 0 until event.pointerCount) {
                    val resp = dispatch(event, i, PHASE_MOVED, screenX, screenY, density, timestampNs)
                    if (resp and RESP_CONSUMED != 0) anyConsumed = true
                    if (resp and RESP_CLAIM != 0) anyClaim = true
                }
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> {
                val idx = event.actionIndex
                val resp = dispatch(event, idx, PHASE_ENDED, screenX, screenY, density, timestampNs)
                if (resp and RESP_CONSUMED != 0) anyConsumed = true
            }
            MotionEvent.ACTION_CANCEL -> {
                for (i in 0 until event.pointerCount) {
                    val resp = dispatch(event, i, PHASE_CANCELLED, screenX, screenY, density, timestampNs)
                    if (resp and RESP_CONSUMED != 0) anyConsumed = true
                }
            }
        }

        if (anyClaim) {
            // Walk up the parent chain telling every ancestor not to
            // intercept this gesture. The flag stays in effect until
            // the next `ACTION_DOWN`. This is the Android analog of
            // iOS's pan-recognizer cancel for the framework's claim
            // protocol.
            v.parent?.requestDisallowInterceptTouchEvent(true)
        }
        return anyConsumed
    }

    private fun dispatch(
        event: MotionEvent,
        pointerIndex: Int,
        phase: Int,
        screenX: Float,
        screenY: Float,
        density: Float,
        timestampNs: Long,
    ): Int {
        val id = event.getPointerId(pointerIndex).toLong()
        // px → dp (screenX/screenY are already dp).
        val x = event.getX(pointerIndex) / density
        val y = event.getY(pointerIndex) / density
        return nativeInvokeTouch(
            nativePtr,
            id,
            phase,
            x, y,
            x + screenX, y + screenY,
            timestampNs,
            event.getPressure(pointerIndex),
        )
    }

    private external fun nativeInvokeTouch(
        ptr: Long,
        id: Long,
        phase: Int,
        x: Float, y: Float,
        winX: Float, winY: Float,
        timestampNs: Long,
        force: Float,
    ): Int

    @Suppress("unused")
    private external fun nativeDrop(ptr: Long)
}
