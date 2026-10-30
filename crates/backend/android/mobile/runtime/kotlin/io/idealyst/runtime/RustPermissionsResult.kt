package io.idealyst.runtime

import java.util.concurrent.ConcurrentHashMap

/**
 * Process-wide registry routing `Activity.onRequestPermissionsResult`
 * callbacks to whichever subsystem called `Activity.requestPermissions`.
 *
 * The runtime-permission answer is delivered only to the host Activity's
 * `onRequestPermissionsResult` override, and SDKs can't subclass or edit the
 * generated `MainActivity`. So `MainActivity.onRequestPermissionsResult`
 * forwards to [dispatch], and an SDK that requests a permission [register]s a
 * handler under the request code it passed to `requestPermissions`. The
 * `permissions` SDK's `RustPermissions` shim is the consumer.
 *
 * The Activity never names an SDK class, so an app that doesn't link an SDK
 * never resolves its JNI symbols. Same shape as [RustActivityResult], with a
 * separate code space: permission and activity-result request codes are
 * independent on Android.
 *
 * Android always answers a `requestPermissions` call. An interrupted prompt
 * arrives as empty `permissions` / `grantResults` arrays, which the handler
 * must read as a cancellation.
 */
object RustPermissionsResult {
    /**
     * Receives one `onRequestPermissionsResult` payload. Return `true` if the
     * handler consumed it. Removed after firing once: each request code
     * answers exactly one `requestPermissions` call.
     */
    fun interface Handler {
        fun onResult(permissions: Array<out String>, grantResults: IntArray): Boolean
    }

    private val handlers = ConcurrentHashMap<Int, Handler>()

    /**
     * Register [handler] for [requestCode]. Replaces any prior handler for the
     * same code, so a stale request is overwritten rather than leaking.
     */
    @JvmStatic
    fun register(requestCode: Int, handler: Handler) {
        handlers[requestCode] = handler
    }

    /** Drop a registered handler without firing it (e.g. a failed launch). */
    @JvmStatic
    fun unregister(requestCode: Int) {
        handlers.remove(requestCode)
    }

    /**
     * Called from `MainActivity.onRequestPermissionsResult`. Routes the result
     * to the handler registered for [requestCode], removing it. Returns `true`
     * if a handler consumed it.
     */
    @JvmStatic
    fun dispatch(requestCode: Int, permissions: Array<out String>, grantResults: IntArray): Boolean {
        val handler = handlers.remove(requestCode) ?: return false
        return handler.onResult(permissions, grantResults)
    }
}
