package io.idealyst.runtime

import android.graphics.SurfaceTexture
import android.view.Surface
import android.view.TextureView

/**
 * `TextureView.SurfaceTextureListener` that forwards every
 * surface-texture lifecycle event to a Rust-side `GraphicsCallback`
 * box via JNI. Sibling of [`RustGraphicsCallback`] (which targets
 * `SurfaceView` + `SurfaceHolder.Callback`); we use `TextureView`
 * for embedded previews where the surface must composite normally
 * inside the View tree (sidebar drawers, modals, popovers
 * overlaying the preview). `SurfaceView` with
 * `setZOrderOnTop(true)` would render above every other View in
 * the window — fine for a fullscreen game, but breaks any UI that
 * needs to sit on top of the preview.
 *
 * Each callback wraps the `SurfaceTexture` in a `Surface` (the
 * Rust side calls `ANativeWindow_fromSurface` on it, same path
 * the SurfaceView listener uses) and forwards the dimensions.
 *
 * Lifetime: `nativePtr` is leaked at construction — see the doc on
 * [`RustGraphicsCallback`] for the matching `nativeDrop` finalizer
 * story.
 */
class RustTextureListener(private val nativePtr: Long) : TextureView.SurfaceTextureListener {
    // Held for the lifetime of the listener so we can release it on
    // destroy. SurfaceTexture itself is owned by TextureView; the
    // Surface wrapper we create here is a Java-side handle the Rust
    // ANativeWindow conversion borrows from. Keeping it in a field
    // matches the SurfaceView path's `holder.surface` ownership
    // semantics — the Holder owns the Surface, we forward a borrow.
    private var surface: Surface? = null

    override fun onSurfaceTextureAvailable(texture: SurfaceTexture, width: Int, height: Int) {
        val s = Surface(texture)
        surface = s
        nativeSurfaceCreated(nativePtr, s)
        // TextureView's `onSurfaceTextureAvailable` carries the
        // initial size, where SurfaceView fires `surfaceCreated`
        // and then immediately `surfaceChanged` with the size.
        // Mirror that by emitting both events here so the Rust
        // side's deferred-on-ready path doesn't need separate
        // listener variants.
        nativeSurfaceChanged(nativePtr, s, width, height)
    }

    override fun onSurfaceTextureSizeChanged(texture: SurfaceTexture, width: Int, height: Int) {
        val s = surface ?: Surface(texture).also { surface = it }
        nativeSurfaceChanged(nativePtr, s, width, height)
    }

    override fun onSurfaceTextureDestroyed(texture: SurfaceTexture): Boolean {
        nativeSurfaceDestroyed(nativePtr)
        surface?.release()
        surface = null
        // Return true: we're done with the SurfaceTexture and
        // TextureView can release it. (false would mean "I'll
        // release it myself," used when the texture is being
        // handed off to another consumer.)
        return true
    }

    override fun onSurfaceTextureUpdated(texture: SurfaceTexture) {
        // No-op. Fires every frame that the producer (us, via wgpu)
        // posts a new buffer to the SurfaceTexture. We don't need
        // a per-frame Rust dispatch — the render-loop driver
        // already drives draw_frame at vsync.
    }

    /**
     * Frees the leaked Rust `Box<GraphicsCallback>` when this Kotlin
     * object is GC'd. Same pattern as [`RustGraphicsCallback`].
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
