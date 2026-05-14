package com.idealyst.runtime

import android.view.View

/**
 * `View.OnClickListener` whose `onClick` dispatches into Rust via a
 * cached native pointer.
 *
 * The backend's `create_button` boxes the click closure, leaks it, and
 * hands the raw pointer to this listener. On every click we trampoline
 * back to `nativeInvoke`, which downcasts the pointer and runs the
 * closure on the UI thread.
 *
 * Lifetime: the pointer is *not* freed when this listener is garbage
 * collected. Wiring `nativeDrop` from `finalize()` would solve that,
 * but the demo Activity outlives every button, so the leak is bounded.
 */
class RustClickListener(private val nativePtr: Long) : View.OnClickListener {
    override fun onClick(v: View?) {
        nativeInvoke(nativePtr)
    }

    private external fun nativeInvoke(ptr: Long)

    @Suppress("unused")
    private external fun nativeDrop(ptr: Long)
}
