package io.idealyst.runtime

import android.content.Intent
import java.util.concurrent.ConcurrentHashMap

/**
 * Process-wide registry routing `Activity.onActivityResult` callbacks to
 * whichever subsystem launched the corresponding `startActivityForResult`.
 *
 * ## Why this exists
 *
 * `startActivityForResult` / `onActivityResult` is the only way to drive a
 * flow that needs a *result* from another Activity — most notably
 * MediaProjection's consent intent
 * (`MediaProjectionManager.createScreenCaptureIntent()`). But the result is
 * delivered to the host Activity's `onActivityResult` override, and SDKs
 * (which ship as plain crates + runtime Kotlin) can't subclass or edit the
 * generated `MainActivity`.
 *
 * So `MainActivity.onActivityResult` does exactly one thing — forward to
 * [dispatch] — and any SDK that needs a result [register]s a handler keyed by
 * the `requestCode` it launched with. The SDK is fully decoupled from the
 * generated Activity: it never touches `MainActivity` directly. The
 * `screen-recorder` SDK's `RustScreenCaptureHelper` is the first consumer.
 *
 * First-party (lives in the backend's `RUNTIME_KOTLIN_FILES`), so it's
 * bundled into every app and available before any SDK loads.
 */
object RustActivityResult {
    /**
     * Receives one `onActivityResult` payload. Return `true` if this handler
     * consumed it (so [dispatch] can report it was handled). The handler is
     * removed after firing once — these flows are single-shot (a consent
     * grant resolves exactly one pending request).
     */
    fun interface Handler {
        fun onResult(resultCode: Int, data: Intent?): Boolean
    }

    private val handlers = ConcurrentHashMap<Int, Handler>()

    /**
     * Register [handler] for [requestCode]. Replaces any prior handler for
     * the same code (a stale, never-fired request is overwritten rather than
     * leaking). Pick a `requestCode` unlikely to collide — SDKs use a large
     * constant namespace (e.g. screen-capture uses `0x5C12`).
     */
    @JvmStatic
    fun register(requestCode: Int, handler: Handler) {
        handlers[requestCode] = handler
    }

    /** Drop a registered handler without firing it (e.g. teardown). */
    @JvmStatic
    fun unregister(requestCode: Int) {
        handlers.remove(requestCode)
    }

    /**
     * Called from `MainActivity.onActivityResult`. Routes the result to the
     * handler registered for [requestCode], removing it (single-shot).
     * Returns `true` if a handler consumed it.
     */
    @JvmStatic
    fun dispatch(requestCode: Int, resultCode: Int, data: Intent?): Boolean {
        val handler = handlers.remove(requestCode) ?: return false
        return handler.onResult(resultCode, data)
    }
}
