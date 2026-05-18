package io.idealyst.runtime

import android.view.Surface
import android.view.SurfaceHolder

/**
 * `SurfaceHolder.Callback` implementation that forwards every
 * surface lifecycle event to a Rust-side `GraphicsCallback` box
 * via JNI. The three native methods downcast the `nativePtr`
 * (a `Box<GraphicsCallback>` raw pointer leaked at construction),
 * convert the Java `Surface` to an `ANativeWindow*` on the Rust
 * side, and dispatch to the user's `on_ready` / `on_resize` /
 * `on_lost` closures.
 *
 * Lifetime: the `nativePtr` is leaked at construction (mirrors
 * `RustClickListener` and friends). The Rust side owns it until
 * `Backend::release_graphics` runs, which calls `nativeDrop` to
 * free it.
 */
class RustGraphicsCallback(private val nativePtr: Long) : SurfaceHolder.Callback {
    override fun surfaceCreated(holder: SurfaceHolder) {
        nativeSurfaceCreated(nativePtr, holder.surface)
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        nativeSurfaceChanged(nativePtr, holder.surface, width, height)
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        nativeSurfaceDestroyed(nativePtr)
    }

    /**
     * Frees the leaked Rust `Box<GraphicsCallback>` when this Kotlin
     * object is GC'd. Standard Java `finalize()` is deprecated, but
     * for this single-Activity demo it's the simplest hook — and it
     * matches the pattern the rest of the runtime uses (e.g.
     * `RustClickListener`'s wired-but-unused `nativeDrop`). A
     * production app would replace this with explicit `PhantomReference`
     * cleanup, but the per-Activity scope here is bounded.
     */
    @Suppress("removal")
    protected fun finalize() {
        if (nativePtr != 0L) {
            nativeDrop(nativePtr)
        }
    }

    private external fun nativeSurfaceCreated(ptr: Long, surface: Surface)
    private external fun nativeSurfaceChanged(ptr: Long, surface: Surface, width: Int, height: Int)
    private external fun nativeSurfaceDestroyed(ptr: Long)
    private external fun nativeDrop(ptr: Long)
}
