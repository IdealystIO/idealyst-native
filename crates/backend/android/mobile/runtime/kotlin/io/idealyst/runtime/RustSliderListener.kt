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
        // Forward only user-driven drags. Programmatic `setProgress`
        // calls (wire-replay landing a fresh value from the AAS
        // server, or a sibling effect writing the same signal) fire
        // this listener with `fromUser = false`; routing those back
        // through `nativeChanged` would push the value at the server,
        // which would re-emit it, causing a feedback loop when
        // user-driven and server-driven writes interleave faster than
        // the wire round-trips. Android tells us exactly which is
        // which via `fromUser` — honor it.
        if (!fromUser) return
        nativeChanged(nativePtr, progress)
    }

    override fun onStartTrackingTouch(seekBar: SeekBar?) {}
    override fun onStopTrackingTouch(seekBar: SeekBar?) {}

    private external fun nativeChanged(ptr: Long, progress: Int)
}
