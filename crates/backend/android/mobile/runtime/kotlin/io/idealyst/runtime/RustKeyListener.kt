package io.idealyst.runtime

import android.view.KeyEvent
import android.view.View
import android.widget.EditText

/**
 * `View.OnKeyListener` that forwards keydown events into Rust via a
 * cached native pointer. The Rust side hands us a raw pointer to a
 * leaked `Box<KeyDownCallback>`; we pass it back on each keydown
 * along with the key code, modifier metaState, the unicode character
 * (zero if the key has no printable representation), and the
 * EditText's current selection range.
 *
 * Only `KeyEvent.ACTION_DOWN` fires the callback — autorepeat and
 * key-up are filtered. This matches the cross-platform contract
 * documented on `KeyOutcome` / `KeyEvent` in `framework_core`: one
 * call per logical keydown, before the platform default runs.
 *
 * Return value semantics:
 * - `true`  → consume the event, suppressing the EditText's default
 *             (matches `KeyOutcome::PreventDefault`).
 * - `false` → let the EditText handle the keydown normally
 *             (matches `KeyOutcome::Default`).
 *
 * The keycode → canonical-name mapping (e.g. `KEYCODE_TAB` → `"Tab"`)
 * is done on the Rust side — keeping the mapping in one place across
 * platforms.
 */
class RustKeyListener(private val nativePtr: Long) : View.OnKeyListener {
    override fun onKey(v: View?, keyCode: Int, event: KeyEvent?): Boolean {
        if (event == null) return false
        if (event.action != KeyEvent.ACTION_DOWN) return false
        // Reading selection here (rather than on the Rust side) avoids
        // an extra JNI round-trip. EditText is the only View we wire
        // this listener to today; a non-EditText fallback returns -1
        // for both bounds, which the Rust side maps to selection_start
        // == selection_end == 0.
        val selStart: Int
        val selEnd: Int
        if (v is EditText) {
            selStart = v.selectionStart
            selEnd = v.selectionEnd
        } else {
            selStart = -1
            selEnd = -1
        }
        val unicode = event.unicodeChar
        return nativeKey(nativePtr, keyCode, event.metaState, unicode, selStart, selEnd)
    }

    private external fun nativeKey(
        ptr: Long,
        keyCode: Int,
        metaState: Int,
        unicodeChar: Int,
        selStart: Int,
        selEnd: Int,
    ): Boolean
}
