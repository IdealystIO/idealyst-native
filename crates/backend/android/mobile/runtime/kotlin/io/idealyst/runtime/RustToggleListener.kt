package io.idealyst.runtime

import android.widget.CompoundButton

/**
 * `CompoundButton.OnCheckedChangeListener` that forwards toggle
 * state into Rust. Used by `Switch` (and any future compound button
 * primitives we add).
 *
 * The `suppress` flag lets the Rust-side `update_value` path write
 * `setChecked(...)` without re-firing back into Rust. Android's
 * `setChecked` invokes the listener for any value change regardless
 * of who initiated it, so without this guard a programmatic update
 * (e.g. an AAS wire-replay landing a fresh `value`) would race
 * against the user's most recent tap: server emits an out-of-date
 * value → client `setChecked` triggers the listener → client sends
 * the value back to server as an event → server re-emits → repeat.
 * The visible symptom is the toggle flipping on its own after a
 * spam-click. Setting `suppress = true` around the programmatic
 * call breaks the cycle without disturbing real user input.
 */
class RustToggleListener(private val nativePtr: Long) :
    CompoundButton.OnCheckedChangeListener {

    @JvmField
    var suppress: Boolean = false

    override fun onCheckedChanged(buttonView: CompoundButton, isChecked: Boolean) {
        if (suppress) return
        nativeChanged(nativePtr, if (isChecked) 1 else 0)
    }

    private external fun nativeChanged(ptr: Long, checked: Int)
}
