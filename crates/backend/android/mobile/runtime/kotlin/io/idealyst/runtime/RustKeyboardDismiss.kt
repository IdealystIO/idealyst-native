package io.idealyst.runtime

import android.content.Context
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import android.view.ViewGroup
import android.view.Window
import android.view.inputmethod.InputMethodManager
import android.widget.EditText

/**
 * A tap outside the focused text input blurs it and closes the soft keyboard:
 * the Android half of iOS's keyboard-dismiss tap recognizer. Android has no
 * platform behavior for this, so an `EditText` otherwise keeps focus until the
 * person closes the keyboard themselves.
 *
 * Fed by [RustActivityWindowCallback], which sees every touch of the window
 * before any view. It OBSERVES: nothing is consumed, so the tap still presses
 * whatever it landed on, and the blur runs after the views have had the
 * `ACTION_UP`.
 *
 * Same rules as iOS:
 *
 * - A TAP, not a touch: a finger that moves past the touch slop is a scroll,
 *   and scrolling a form keeps the keyboard up.
 * - A tap on another text input is skipped, so moving between fields is a
 *   clean native handoff rather than a dismiss and refocus.
 * - A tap inside a focus-preserving region ([markPreservesFocus],
 *   `InputOps::mark_preserves_focus`) is skipped, so picking an option from a
 *   combobox's menu keeps the input it belongs to.
 *
 * `BlurOutcome::Keep` still holds: [RustFocusListener] takes focus back and
 * reopens the keyboard.
 *
 * UI thread only.
 */
object RustKeyboardDismiss {
    /**
     * The `View.setTag(int, Object)` key marking a focus-preserving region.
     * Not a generated resource id, since this runtime has none. `setTag` only
     * refuses keys below `0x02000000`, which are the framework's and system's.
     */
    private const val PRESERVES_FOCUS_TAG = 0x1DEAF0C5

    /** The focused input a tap in progress would blur; null when none would. */
    private var armed: EditText? = null
    private var downX = 0f
    private var downY = 0f
    private val location = IntArray(2)

    /** `InputOps::mark_preserves_focus`. */
    @JvmStatic
    fun markPreservesFocus(view: View) {
        view.setTag(PRESERVES_FOCUS_TAG, true)
    }

    fun beforeDispatch(window: Window, event: MotionEvent) {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                downX = event.rawX
                downY = event.rawY
                val focused = window.currentFocus as? EditText
                armed = focused?.takeUnless { landsOnKept(window.decorView, downX, downY) }
            }
            MotionEvent.ACTION_MOVE -> {
                val field = armed ?: return
                val slop = ViewConfiguration.get(field.context).scaledTouchSlop
                val dx = event.rawX - downX
                val dy = event.rawY - downY
                if (dx * dx + dy * dy > slop * slop) armed = null
            }
            MotionEvent.ACTION_POINTER_DOWN, MotionEvent.ACTION_CANCEL -> armed = null
        }
    }

    fun afterDispatch(event: MotionEvent) {
        if (event.actionMasked != MotionEvent.ACTION_UP) return
        val field = armed ?: return
        armed = null
        // The press may have moved focus itself.
        if (field.isFocused) blur(field)
    }

    /** Whether a text input or a focus-preserving region is under the point. */
    private fun landsOnKept(view: View, x: Float, y: Float): Boolean {
        if (view.visibility != View.VISIBLE) return false
        view.getLocationOnScreen(location)
        val inside = x >= location[0] && x < location[0] + view.width &&
            y >= location[1] && y < location[1] + view.height
        if (!inside) return false
        if (view is EditText || view.getTag(PRESERVES_FOCUS_TAG) != null) return true
        if (view is ViewGroup) {
            for (i in view.childCount - 1 downTo 0) {
                if (landsOnKept(view.getChildAt(i), x, y)) return true
            }
        }
        return false
    }

    /** Same steps as `blur_edit_text` in `imp/primitives/text_input.rs`. */
    private fun blur(field: EditText) {
        val token = field.windowToken
        field.clearFocus()
        val imm = field.context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
        imm?.hideSoftInputFromWindow(token, 0)
    }
}
