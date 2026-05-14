package io.idealyst.runtime

import android.widget.SeekBar

/**
 * `SeekBar.OnSeekBarChangeListener` that forwards the integer
 * progress value into Rust. The Rust side has stashed the original
 * [min, max] f32 range alongside the callback, so it can map the
 * integer back to user-space.
 *
 * Only `onProgressChanged` fires the callback — start/stop tracking
 * events are unused for the controlled-input pattern.
 */
class RustSliderListener(private val nativePtr: Long) :
    SeekBar.OnSeekBarChangeListener {

    override fun onProgressChanged(seekBar: SeekBar?, progress: Int, fromUser: Boolean) {
        // We forward every change — drag from user *and* programmatic
        // setProgress calls. Rust's controlled-update path
        // short-circuits identical values, so re-entry is fine.
        nativeChanged(nativePtr, progress)
    }

    override fun onStartTrackingTouch(seekBar: SeekBar?) {}
    override fun onStopTrackingTouch(seekBar: SeekBar?) {}

    private external fun nativeChanged(ptr: Long, progress: Int)
}
