package io.idealyst.runtime

import android.view.View

/**
 * Attach notification for the attach-safe `focus()` (the Rust side is
 * `backend-android-mobile`'s `imp/pending_focus.rs`).
 *
 * `requestFocus()` on an `EditText` that is not attached to a window does
 * not take, and realize builds a subtree before inserting it (a modal's
 * content attaches later still). Rust records the pending focus and installs
 * one of these on the field; on attach it removes itself and calls back on
 * the NEXT main-loop turn (`post`): the attach runs inside the backend's
 * `addView`, and focusing there would fire the focus listener (a style
 * re-apply) while the backend is still mid-insert.
 *
 * Holds no state: Rust decides, from its pending slot, whether this view is
 * still the one to focus (a later focus elsewhere, or a `blur()`, cancels).
 */
class RustAttachFocus : View.OnAttachStateChangeListener {
    override fun onViewAttachedToWindow(v: View) {
        v.removeOnAttachStateChangeListener(this)
        v.post { nativeAttached(v) }
    }

    override fun onViewDetachedFromWindow(v: View) {}

    private external fun nativeAttached(v: View)
}
