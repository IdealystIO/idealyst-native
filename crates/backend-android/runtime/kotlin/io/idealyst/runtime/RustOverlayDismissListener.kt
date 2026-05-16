package io.idealyst.runtime

import android.content.DialogInterface

/**
 * `DialogInterface.OnCancelListener` whose `onCancel` dispatches into
 * Rust via a cached native pointer. Wired to a `Dialog` so that:
 *
 *  - tap on the scrim outside the content (when `canceledOnTouchOutside`
 *    is enabled on the dialog) triggers cancel;
 *  - hardware/gesture back-button triggers cancel.
 *
 * Both paths converge on `OnCancelListener.onCancel`, then this trampoline,
 * then Rust's `on_dismiss` user closure. The framework's host code then
 * flips its open-state signal, the surrounding `when` rebuilds, and the
 * overlay's enclosing scope drops — which calls `release_overlay` to
 * actually dismiss the dialog and free the leaked callback.
 *
 * The pointer is *not* freed when this listener is GC'd; `nativeDrop` is
 * exposed for parity with the other listener classes but the demo
 * Activity outlives every overlay so the leak is bounded.
 */
class RustOverlayDismissListener(private val nativePtr: Long) : DialogInterface.OnCancelListener {
    override fun onCancel(dialog: DialogInterface?) {
        nativeDismiss(nativePtr)
    }

    private external fun nativeDismiss(ptr: Long)

    @Suppress("unused")
    private external fun nativeDrop(ptr: Long)
}
