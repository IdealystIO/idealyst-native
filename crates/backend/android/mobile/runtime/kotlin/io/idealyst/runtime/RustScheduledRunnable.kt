package io.idealyst.runtime

/**
 * `Runnable` whose `run` forwards into Rust via a leaked
 * `Box<dyn FnOnce()>` pointer. The Rust scheduler instantiates
 * this, calls `handler.postDelayed(this, delayMs)`, and stashes
 * the instance in its [`ScheduleHandle`] so cancel can call
 * `handler.removeCallbacks(this)`.
 *
 * One-shot — `nativeInvoke` consumes the boxed closure. If the
 * runnable is cancelled before firing, the Rust side calls
 * `nativeDrop` to release the box without invoking.
 */
class RustScheduledRunnable(private val nativePtr: Long) : Runnable {
    override fun run() {
        nativeInvoke(nativePtr)
    }

    private external fun nativeInvoke(ptr: Long)

    @Suppress("unused")
    private external fun nativeDrop(ptr: Long)
}
