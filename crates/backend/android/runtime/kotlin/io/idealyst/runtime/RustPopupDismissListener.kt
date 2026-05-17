package io.idealyst.runtime

import android.widget.PopupWindow

/**
 * `PopupWindow.OnDismissListener` whose `onDismiss` dispatches into
 * Rust via a cached native pointer.
 *
 * Used by the Android `Overlay` backend for *element-anchored*
 * overlays (popovers / tooltips / dropdowns), which are backed by
 * `PopupWindow` rather than `Dialog`. The Rust trampoline is shared
 * with `RustOverlayDismissListener` — both classes call
 * `Java_io_idealyst_runtime_RustOverlayDismissListener_nativeDismiss`
 * via the same native pointer convention. We need a *separate Kotlin
 * class* only because `PopupWindow.OnDismissListener` and
 * `DialogInterface.OnCancelListener` have different method names and
 * signatures.
 *
 * Important difference from the Dialog flow: `PopupWindow.dismiss()`
 * fires `onDismiss()` regardless of who initiated it (user tap-outside
 * OR programmatic dismissal from `release_overlay`). The Rust side
 * blanks its `OverlayDismissCallback.inner` slot before calling
 * `popup.dismiss()`, so the trampoline becomes a no-op for the
 * framework-driven path. Only user-initiated dismissals end up
 * firing the host's `on_dismiss` closure.
 */
class RustPopupDismissListener(private val nativePtr: Long) : PopupWindow.OnDismissListener {
    override fun onDismiss() {
        nativeDismiss(nativePtr)
    }

    private external fun nativeDismiss(ptr: Long)
}
