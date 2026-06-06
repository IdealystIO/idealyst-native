package io.idealyst.runtime

import android.view.View

/**
 * `View.OnLayoutChangeListener` attached to the Activity root by the Rust
 * backend ([AndroidBackend.install_viewport_resize_listener]). Fires
 * `nativeViewportResized` whenever the root's SIZE changes — which includes
 * the soft keyboard (IME) opening AND closing (the window resizes in
 * adjustResize / edge-to-edge mode). The Rust side schedules a layout pass so
 * the framework's reactive viewport re-mirrors the new size.
 *
 * Without this, the layout stays stuck at the keyboard-open (shrunk) size
 * after the keyboard closes: the OPEN is covered incidentally by the busy
 * focus/IME animation driving layout passes, but the quiet CLOSE has nothing
 * else driving a pass, so the restored `getHeight()` is never re-read. Hooking
 * the root's layout change makes open and close symmetric.
 *
 * Only calls native on an actual width/height change (not on every layout) to
 * avoid redundant passes; the Rust scheduler additionally coalesces them.
 */
class RustViewportResizeListener : View.OnLayoutChangeListener {
    override fun onLayoutChange(
        v: View?,
        left: Int,
        top: Int,
        right: Int,
        bottom: Int,
        oldLeft: Int,
        oldTop: Int,
        oldRight: Int,
        oldBottom: Int,
    ) {
        val sizeChanged =
            (right - left) != (oldRight - oldLeft) || (bottom - top) != (oldBottom - oldTop)
        if (sizeChanged) {
            nativeViewportResized()
        }
    }

    private external fun nativeViewportResized()
}
