package io.idealyst.runtime

import android.content.Context
import android.view.KeyEvent
import android.widget.EditText

/**
 * The `EditText` behind `text_input` / `text_area`
 * (`imp/primitives/text_input.rs`): a plain `EditText` except that it leaves
 * an unmodified Escape alone.
 *
 * From API 37, `TextView.doKeyDown` takes Escape itself: with focus, it
 * clears the field's focus, hides the keyboard and reports the key handled.
 * The key then never travels on, so it cannot do what Escape does in a
 * field on every other backend (and on Android up to API 36): reach the
 * enclosing modal's `RustModalOverlay`, which closes the overlay, or the
 * app's key listeners. A modal whose field held focus stayed open, with the
 * field blurred.
 *
 * The field's `on_key_down` still sees Escape first: a `View.OnKeyListener`
 * (`RustKeyListener`) runs before `onKeyDown`, so an author who claims
 * Escape (closing a suggestion list, say) still stops it there.
 */
class RustEditText(context: Context) : EditText(context) {
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean =
        if (passesOn(keyCode, event)) false else super.onKeyDown(keyCode, event)

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean =
        if (passesOn(keyCode, event)) false else super.onKeyUp(keyCode, event)

    private fun passesOn(keyCode: Int, event: KeyEvent): Boolean =
        keyCode == KeyEvent.KEYCODE_ESCAPE && event.hasNoModifiers()
}
