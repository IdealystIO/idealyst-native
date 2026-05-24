package io.idealyst.runtime

import android.text.Editable
import android.text.TextWatcher

/**
 * `TextWatcher` that forwards `afterTextChanged` into Rust via a
 * cached native pointer. The Rust side hands us a raw pointer to a
 * leaked `Box<TextChangeCallback>`; we pass it back on each text
 * change along with the new string content.
 *
 * Only `afterTextChanged` fires the callback — `beforeTextChanged`
 * and `onTextChanged` are no-ops. afterTextChanged runs after the
 * editable buffer is finalized, which matches the controlled-input
 * "single source of truth update" semantics.
 */
class RustTextWatcher(private val nativePtr: Long) : TextWatcher {
    /**
     * Set to `true` around a programmatic `setText(...)` from Rust
     * (see `update_value` in `text_input.rs`). `TextWatcher` doesn't
     * distinguish programmatic vs. user-typed writes; without this
     * guard, an runtime-server wire-replay landing a fresh value from the
     * server would `setText` → fire `afterTextChanged` →
     * `nativeChanged` → another `EventOccurred` back to the server →
     * server emits again. With fast input (keystrokes faster than
     * the wire round-trips), the cycle compounds and the field's
     * contents oscillate between server-state and user-state until
     * the event queue drains.
     */
    @JvmField
    var suppress: Boolean = false

    override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) {}
    override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) {}
    override fun afterTextChanged(s: Editable?) {
        if (suppress) return
        nativeChanged(nativePtr, s?.toString() ?: "")
    }

    private external fun nativeChanged(ptr: Long, text: String)
}
