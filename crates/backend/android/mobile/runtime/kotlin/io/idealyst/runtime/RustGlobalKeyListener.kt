package io.idealyst.runtime

import android.view.KeyEvent
import android.view.View

/**
 * App-level `View.OnKeyListener` attached to the Activity root by the Rust
 * backend's `set_app_key_handler`. Unlike [RustKeyListener] (per-`EditText`,
 * focus-scoped), this fires for every hardware key press while the root holds
 * focus — i.e. whenever no text input is focused — so the app can bind global
 * shortcuts (arrow keys, +/-, …).
 *
 * Fires on `ACTION_DOWN` (matching the web/desktop `keydown` semantics the
 * framework normalizes to). Returns `true` to consume the event when the Rust
 * handler claims it (`KeyOutcome::PreventDefault`), else `false` so normal key
 * routing continues.
 *
 * No `EditText` selection is read here (there's no associated input) — the Rust
 * side passes a 0 selection range.
 */
class RustGlobalKeyListener(private val nativePtr: Long) : View.OnKeyListener {
    override fun onKey(v: View?, keyCode: Int, event: KeyEvent?): Boolean {
        if (event == null || event.action != KeyEvent.ACTION_DOWN) return false
        return nativeGlobalKey(nativePtr, keyCode, event.metaState, event.unicodeChar)
    }

    private external fun nativeGlobalKey(
        ptr: Long,
        keyCode: Int,
        metaState: Int,
        unicodeChar: Int,
    ): Boolean
}
