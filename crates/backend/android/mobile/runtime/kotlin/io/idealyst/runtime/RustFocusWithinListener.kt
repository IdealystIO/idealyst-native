package io.idealyst.runtime

import android.view.View
import android.view.ViewTreeObserver

/**
 * A `view` / `pressable` `on_focus` (the Rust side is
 * `backend-android-mobile`'s `imp/focus_within.rs`): reports whether the
 * window's focused view is [target] or inside it, on every focus change in
 * the window, through `nativeFocusWithin`.
 *
 * Window-wide (`ViewTreeObserver.OnGlobalFocusChangeListener`), NOT the
 * target's `OnFocusChangeListener`: that one slot only hears the view's
 * OWN focus — never a descendant's — and is already shared by
 * `RustFocusListener` (text fields) and `RustStateListener` (FOCUSED).
 *
 * Android's touch mode is the keyboard-focus gate: a tap focuses only
 * views that are focusable in touch mode (text fields), so a tapped button
 * takes no focus and reports nothing; keyboard / D-pad navigation leaves
 * touch mode and focuses buttons too. That matches `:focus-visible`.
 *
 * Registered with the window's observer while [target] is attached
 * (re-registered on re-attach, after a navigator swaps it back in); a
 * detach reports `false`. Reports repeat (`false` for every focus change
 * elsewhere); the vocabulary dedups to transitions.
 */
class RustFocusWithinListener(private val nativePtr: Long, private val target: View) :
    ViewTreeObserver.OnGlobalFocusChangeListener,
    View.OnAttachStateChangeListener {

    init {
        target.addOnAttachStateChangeListener(this)
        if (target.isAttachedToWindow) {
            target.viewTreeObserver.addOnGlobalFocusChangeListener(this)
        }
    }

    override fun onViewAttachedToWindow(v: View) {
        v.viewTreeObserver.addOnGlobalFocusChangeListener(this)
    }

    override fun onViewDetachedFromWindow(v: View) {
        v.viewTreeObserver.removeOnGlobalFocusChangeListener(this)
        nativeFocusWithin(nativePtr, false)
    }

    override fun onGlobalFocusChanged(oldFocus: View?, newFocus: View?) {
        nativeFocusWithin(nativePtr, contains(newFocus))
    }

    private fun contains(focused: View?): Boolean {
        var cur: View? = focused
        while (cur != null) {
            if (cur === target) return true
            cur = cur.parent as? View
        }
        return false
    }

    private external fun nativeFocusWithin(ptr: Long, focused: Boolean)
}
