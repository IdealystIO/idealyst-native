package io.idealyst.runtime

import android.widget.CompoundButton

/**
 * `CompoundButton.OnCheckedChangeListener` that forwards toggle
 * state into Rust. Used by `Switch` (and any future compound button
 * primitives we add).
 */
class RustToggleListener(private val nativePtr: Long) :
    CompoundButton.OnCheckedChangeListener {

    override fun onCheckedChanged(buttonView: CompoundButton?, isChecked: Boolean) {
        nativeChanged(nativePtr, if (isChecked) 1 else 0)
    }

    private external fun nativeChanged(ptr: Long, checked: Int)
}
