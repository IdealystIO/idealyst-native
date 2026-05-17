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
    override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) {}
    override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) {}
    override fun afterTextChanged(s: Editable?) {
        nativeChanged(nativePtr, s?.toString() ?: "")
    }

    private external fun nativeChanged(ptr: Long, text: String)
}
