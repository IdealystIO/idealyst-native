package io.idealyst.runtime

import android.view.KeyEvent
import android.view.View
import android.view.ViewTreeObserver

/**
 * App-level keyboard source attached to the Activity root by the Rust
 * backend's `set_keyboard_sink` (`imp/keyboard.rs`). Unlike [RustKeyListener]
 * (per-`EditText`, key-down only), this reports every hardware key DOWN and UP
 * the root receives — modifier keys included — so a game can track held keys.
 *
 * - `View.OnKeyListener` on the root: `ACTION_DOWN` / `ACTION_UP` →
 *   `nativeGlobalKey(ptr, up, repeat, keyCode, scanCode, metaState,
 *   unicodeChar)`. `scanCode` is the physical key (Rust maps it to the Web
 *   `KeyboardEvent.code`); `unicodeChar` is valid on both phases. Returns
 *   `true` (consume) when Rust answers `KeyOutcome::PreventDefault`.
 * - Focus loss → `nativeKeyFocusLost(ptr)`: the root only receives keys while
 *   it holds focus, and the window only while it has window focus. When focus
 *   moves off the root (a text input or modal overlay took it) or the window
 *   loses focus (app switch, notification shade, a system dialog), the
 *   releases of keys still held will never arrive here, so Rust synthesizes
 *   them.
 *
 * A focused text input keeps its own keys (they go to the input, not the
 * root) — app-level controls must not eat typing.
 *
 * Lifecycle: Rust constructs it with the pointer to a boxed `KeyboardSink`,
 * calls [attach], and calls [detach] BEFORE freeing the box. [detach] zeroes
 * [nativePtr], so a callback already queued on the looper is a no-op instead
 * of a use-after-free. All calls happen on the UI thread.
 *
 * The native signatures must match `imp/jni_exports.rs` exactly: JNI binds
 * `nativeGlobalKey` / `nativeKeyFocusLost` by NAME, so a drifted parameter
 * list would not fail to link — it would pass garbage. This file ships with
 * the crate's Rust code (the CLI stages it from the same resolved
 * `backend-android-mobile`), so the two always change together.
 */
class RustGlobalKeyListener(private var nativePtr: Long) :
    View.OnKeyListener,
    ViewTreeObserver.OnWindowFocusChangeListener,
    ViewTreeObserver.OnGlobalFocusChangeListener {

    private var root: View? = null
    private var observer: ViewTreeObserver? = null

    fun attach(root: View) {
        this.root = root
        root.setOnKeyListener(this)
        val vto = root.viewTreeObserver
        vto.addOnWindowFocusChangeListener(this)
        vto.addOnGlobalFocusChangeListener(this)
        observer = vto
        // A `View.OnKeyListener` only fires while the view holds focus; make
        // the root focusable (in touch mode) and take focus so app-level keys
        // land here when nothing else is focused. Don't steal focus from a
        // view that already holds it (a focused text input keeps its keys).
        root.isFocusableInTouchMode = true
        if (root.findFocus() == null) root.requestFocus()
    }

    fun detach() {
        nativePtr = 0L
        val r = root ?: return
        root = null
        r.setOnKeyListener(null)
        // Remove from the observer we registered on; if the view was
        // re-attached since, its listeners were merged into a new observer,
        // so try the current one as well. A listener left behind is inert
        // (nativePtr is 0).
        for (vto in listOfNotNull(observer, r.viewTreeObserver)) {
            if (vto.isAlive) {
                vto.removeOnWindowFocusChangeListener(this)
                vto.removeOnGlobalFocusChangeListener(this)
            }
        }
        observer = null
    }

    override fun onKey(v: View?, keyCode: Int, event: KeyEvent?): Boolean {
        val ptr = nativePtr
        if (event == null || ptr == 0L) return false
        val up = when (event.action) {
            KeyEvent.ACTION_DOWN -> false
            KeyEvent.ACTION_UP -> true
            else -> return false // ACTION_MULTIPLE: deprecated, not a key transition
        }
        return nativeGlobalKey(
            ptr,
            up,
            event.repeatCount > 0,
            keyCode,
            event.scanCode,
            event.metaState,
            event.unicodeChar,
        )
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        val ptr = nativePtr
        if (!hasFocus && ptr != 0L) nativeKeyFocusLost(ptr)
    }

    override fun onGlobalFocusChanged(oldFocus: View?, newFocus: View?) {
        val ptr = nativePtr
        val r = root ?: return
        if (ptr != 0L && oldFocus === r && newFocus !== r) nativeKeyFocusLost(ptr)
    }

    private external fun nativeGlobalKey(
        ptr: Long,
        up: Boolean,
        repeat: Boolean,
        keyCode: Int,
        scanCode: Int,
        metaState: Int,
        unicodeChar: Int,
    ): Boolean

    private external fun nativeKeyFocusLost(ptr: Long)
}
