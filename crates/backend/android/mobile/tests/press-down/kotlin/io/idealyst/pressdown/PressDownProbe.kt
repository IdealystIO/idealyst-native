package io.idealyst.pressdown

import android.content.Context
import android.os.SystemClock
import android.util.Log
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.view.accessibility.AccessibilityNodeInfo
import android.widget.FrameLayout
import io.idealyst.runtime.RustPressDownLayout

/**
 * `RustPressDownLayout` driven directly: each case builds a fresh layout
 * inside a [RecordingParent], dispatches synthesized events through the
 * parent (so they hit-test into the layout as real touches do), and counts
 * `OnClickListener` calls — the slot the backend fills with
 * `RustClickListener`. Covers what `adb input` cannot inject: a secondary
 * mouse button, a mouse press with no button reported, TalkBack's
 * ACTION_CLICK, a disabled layout. Logs `[press-down]` lines for `run.sh`.
 */
object PressDownProbe {
    /** Records what reaches the ancestor of the press-down layout. */
    private class RecordingParent(context: Context) : FrameLayout(context) {
        var touchEvents = 0
        var disallowedIntercept = false

        override fun onTouchEvent(event: MotionEvent): Boolean {
            touchEvents++
            return true
        }

        override fun requestDisallowInterceptTouchEvent(disallow: Boolean) {
            if (disallow) disallowedIntercept = true
            super.requestDisallowInterceptTouchEvent(disallow)
        }
    }

    private class Rig(context: Context) {
        val parent = RecordingParent(context)
        val layout = RustPressDownLayout(context)
        var clicks = 0

        init {
            layout.isClickable = true
            layout.isFocusable = true
            layout.setOnClickListener { clicks++ }
            parent.addView(layout, ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
            val spec = View.MeasureSpec.makeMeasureSpec(SIZE, View.MeasureSpec.EXACTLY)
            parent.measure(spec, spec)
            parent.layout(0, 0, SIZE, SIZE)
        }

        private val downTime = SystemClock.uptimeMillis()

        fun touch(action: Int, x: Float = 10f, y: Float = 10f, tool: Int = MotionEvent.TOOL_TYPE_FINGER, buttons: Int = 0) {
            val props = MotionEvent.PointerProperties().apply { id = 0; toolType = tool }
            val coords = MotionEvent.PointerCoords().apply { this.x = x; this.y = y; pressure = 1f; size = 1f }
            val source = if (tool == MotionEvent.TOOL_TYPE_MOUSE) InputDevice.SOURCE_MOUSE else InputDevice.SOURCE_TOUCHSCREEN
            val ev = MotionEvent.obtain(
                downTime, SystemClock.uptimeMillis(), action, 1, arrayOf(props), arrayOf(coords),
                0, buttons, 1f, 1f, 0, 0, source, 0,
            )
            parent.dispatchTouchEvent(ev)
            ev.recycle()
        }

        // `onKeyDown`/`onKeyUp` directly: `ViewGroup.dispatchKeyEvent` only
        // delivers to a FOCUSED group, and this detached rig can't take
        // focus. These are the methods dispatch reaches on a focused view,
        // where `View` turns a confirm key into `performClick`.
        fun key(code: Int) {
            layout.onKeyDown(code, KeyEvent(KeyEvent.ACTION_DOWN, code))
            layout.onKeyUp(code, KeyEvent(KeyEvent.ACTION_UP, code))
        }
    }

    private const val SIZE = 200
    private var failures = 0
    private var checks = 0

    private fun check(name: String, ok: Boolean, detail: String) {
        checks++
        if (ok) {
            Log.i("idealyst", "[press-down] PASS $name")
        } else {
            failures++
            Log.i("idealyst", "[press-down] FAIL $name ($detail)")
        }
    }

    @JvmStatic
    fun run(context: Context): Int {
        failures = 0
        checks = 0

        Rig(context).apply {
            touch(MotionEvent.ACTION_DOWN)
            check("a finger down fires once, at the down", clicks == 1, "clicks=$clicks")
            check("the down shows the layout pressed", layout.isPressed, "isPressed=false")
            check("the down keeps ancestors from intercepting the press", parent.disallowedIntercept, "no requestDisallowInterceptTouchEvent")
            touch(MotionEvent.ACTION_MOVE, x = 30f, y = 40f)
            touch(MotionEvent.ACTION_MOVE, x = SIZE + 50f, y = 10f)
            touch(MotionEvent.ACTION_UP, x = SIZE + 50f, y = 10f)
            check("the moves and the release fire nothing more", clicks == 1, "clicks=$clicks")
            check("the release clears pressed", !layout.isPressed, "isPressed=true")
            check("no event of the press reaches the ancestor", parent.touchEvents == 0, "ancestor onTouchEvent x${parent.touchEvents}")
        }

        Rig(context).apply {
            touch(MotionEvent.ACTION_DOWN)
            touch(MotionEvent.ACTION_CANCEL)
            check("a cancelled press fired once and clears pressed", clicks == 1 && !layout.isPressed, "clicks=$clicks pressed=${layout.isPressed}")
        }

        Rig(context).apply {
            touch(MotionEvent.ACTION_DOWN, tool = MotionEvent.TOOL_TYPE_MOUSE, buttons = MotionEvent.BUTTON_SECONDARY)
            touch(MotionEvent.ACTION_UP, tool = MotionEvent.TOOL_TYPE_MOUSE)
            check("a secondary mouse press fires nothing", clicks == 0, "clicks=$clicks")
            check("a secondary mouse press does not reach the ancestor", parent.touchEvents == 0, "ancestor onTouchEvent x${parent.touchEvents}")
            touch(MotionEvent.ACTION_DOWN, tool = MotionEvent.TOOL_TYPE_MOUSE, buttons = MotionEvent.BUTTON_PRIMARY)
            touch(MotionEvent.ACTION_UP, tool = MotionEvent.TOOL_TYPE_MOUSE)
            check("a primary mouse press fires once", clicks == 1, "clicks=$clicks")
            touch(MotionEvent.ACTION_DOWN, tool = MotionEvent.TOOL_TYPE_MOUSE, buttons = 0)
            touch(MotionEvent.ACTION_UP, tool = MotionEvent.TOOL_TYPE_MOUSE)
            check("a mouse press reporting no button (injected tap) fires once", clicks == 2, "clicks=$clicks")
        }

        Rig(context).apply {
            layout.performClick()
            check("performClick (PressableOps::click) fires once", clicks == 1, "clicks=$clicks")
            key(KeyEvent.KEYCODE_DPAD_CENTER)
            check("DPAD_CENTER fires once", clicks == 2, "clicks=$clicks")
            key(KeyEvent.KEYCODE_ENTER)
            check("ENTER fires once", clicks == 3, "clicks=$clicks")
            layout.performAccessibilityAction(AccessibilityNodeInfo.ACTION_CLICK, null)
            check("TalkBack ACTION_CLICK fires once", clicks == 4, "clicks=$clicks")
        }

        Rig(context).apply {
            layout.isEnabled = false
            touch(MotionEvent.ACTION_DOWN)
            touch(MotionEvent.ACTION_UP)
            check("a disabled layout fires nothing", clicks == 0, "clicks=$clicks")
        }

        Log.i("idealyst", "[press-down] DONE $checks checks, $failures failed")
        return failures
    }
}
