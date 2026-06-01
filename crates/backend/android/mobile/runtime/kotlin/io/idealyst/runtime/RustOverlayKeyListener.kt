package io.idealyst.runtime

import android.view.KeyEvent
import android.view.View

/**
 * `View.OnKeyListener` for view-overlay portals (modals). Replaces the
 * hardware/gesture back-button dismissal that an `android.app.Dialog`
 * used to give for free via `setOnCancelListener`.
 *
 * The portal is now a plain overlay `View` reparented into the Activity
 * root (no separate window), so there's no `Dialog` to route back into.
 * We instead make the modal overlay focusable-in-touch-mode and attach
 * this listener; when the overlay (or a descendant) holds focus and the
 * back key is released, we trampoline into Rust's `on_dismiss` — the
 * same `nativeDismiss` entry point the old `OnCancelListener` used, so
 * the Rust side is unchanged.
 *
 * We fire on `ACTION_UP` (not `ACTION_DOWN`) to match the platform's
 * own dialog-cancel timing and to avoid a double-dispatch when the key
 * repeats. Returning `true` consumes the event so it doesn't fall
 * through to the Activity's own back handling (which would pop a
 * navigator screen *and* dismiss the modal).
 *
 * Non-modal overlays (toast host, banners) do NOT attach this listener
 * and stay non-focusable, so back falls through to the app as before.
 */
class RustOverlayKeyListener(private val nativePtr: Long) : View.OnKeyListener {
    override fun onKey(v: View?, keyCode: Int, event: KeyEvent?): Boolean {
        if (event == null) return false
        if (keyCode == KeyEvent.KEYCODE_BACK && event.action == KeyEvent.ACTION_UP) {
            nativeDismiss(nativePtr)
            return true
        }
        return false
    }

    // Reuses the same Rust trampoline as RustOverlayDismissListener — the
    // dismiss semantics are identical, only the JVM-side trigger differs.
    private external fun nativeDismiss(ptr: Long)
}
