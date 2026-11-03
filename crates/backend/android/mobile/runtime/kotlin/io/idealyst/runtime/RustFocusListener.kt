package io.idealyst.runtime

import android.content.Context
import android.view.View
import android.view.inputmethod.InputMethodManager

/**
 * A text field's focus listener (the Rust side is `backend-android-mobile`'s
 * `imp/text_focus.rs` + `text_focus_policy.rs`): every focus change goes to
 * `nativeFocusChanged`, which runs the field's `on_focus(bool)` and, on a
 * loss, its cancelable `on_blur`.
 *
 * A `View` has ONE `OnFocusChangeListener` slot, and the field's style
 * states (`RustStateListener`, `FOCUSED`) need it too. This listener holds
 * the slot and forwards to the other one ([chainOther]); installing either
 * in either order keeps both.
 *
 * `nativeFocusChanged` returns `true` when `on_blur` answered `Keep`. Android
 * has no way to refuse a focus loss, so the field takes focus back on the
 * next main-loop turn, once the change has settled, and only if no other
 * view took it: `Keep` vetoes a drop to no focus, not a move to another
 * field (the `BlurOutcome` contract).
 */
class RustFocusListener(val nativePtr: Long) : View.OnFocusChangeListener {
    private var other: View.OnFocusChangeListener? = null

    override fun onFocusChange(v: View, hasFocus: Boolean) {
        other?.onFocusChange(v, hasFocus)
        if (nativeFocusChanged(nativePtr, hasFocus) && !hasFocus) {
            v.post {
                if (v.isAttachedToWindow && v.rootView.findFocus() == null && v.requestFocus()) {
                    val imm = v.context.getSystemService(Context.INPUT_METHOD_SERVICE) as? InputMethodManager
                    imm?.showSoftInput(v, InputMethodManager.SHOW_IMPLICIT)
                }
            }
        }
    }

    private external fun nativeFocusChanged(ptr: Long, hasFocus: Boolean): Boolean

    companion object {
        /** Make a `RustFocusListener` for `nativePtr` the view's focus listener, keeping the other one. */
        @JvmStatic
        fun install(view: View, nativePtr: Long) {
            val current = view.onFocusChangeListener
            val listener = RustFocusListener(nativePtr)
            listener.other = if (current is RustFocusListener) current.other else current
            view.onFocusChangeListener = listener
        }

        /** Set the view's NON-field focus listener without displacing a `RustFocusListener`. */
        @JvmStatic
        fun chainOther(view: View, listener: View.OnFocusChangeListener) {
            val current = view.onFocusChangeListener
            if (current is RustFocusListener) {
                current.other = listener
            } else {
                view.onFocusChangeListener = listener
            }
        }
    }
}
