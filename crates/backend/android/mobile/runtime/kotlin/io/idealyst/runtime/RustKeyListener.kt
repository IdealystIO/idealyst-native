package io.idealyst.runtime

import android.view.KeyEvent
import android.view.View
import android.widget.EditText
import android.widget.TextView

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
 * documented on `KeyOutcome` / `KeyEvent` in `runtime_core`: one
 * call per logical keydown, before the platform default runs.
 *
 * Also the `TextView.OnEditorActionListener` of a single-line
 * `text_input`: there the soft keyboard's Enter arrives as an editor
 * action (no `KeyEvent`), and is reported as Enter so `on_key_down` sees
 * Return on Android as it does on iOS. An action that DOES carry a
 * `KeyEvent` is a hardware Enter, already reported through `onKey` —
 * forwarding it again would deliver Enter twice.
 *
 * Return value semantics (both entry points):
 * - `true`  → consume the event, suppressing the EditText's default
 *             (matches `KeyOutcome::PreventDefault`; for an editor
 *             action that keeps the IME's Done from hiding the keyboard).
 * - `false` → let the EditText handle it normally
 *             (matches `KeyOutcome::Default`).
 *
 * The keycode → canonical-name mapping (e.g. `KEYCODE_TAB` → `"Tab"`)
 * is done on the Rust side — keeping the mapping in one place across
 * platforms.
 */
class RustKeyListener(private val nativePtr: Long) :
    View.OnKeyListener, TextView.OnEditorActionListener {

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

    override fun onEditorAction(v: TextView?, actionId: Int, event: KeyEvent?): Boolean {
        if (event != null) return false
        val selStart = v?.selectionStart ?: -1
        val selEnd = v?.selectionEnd ?: -1
        return nativeKey(nativePtr, KeyEvent.KEYCODE_ENTER, 0, '\n'.code, selStart, selEnd)
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
