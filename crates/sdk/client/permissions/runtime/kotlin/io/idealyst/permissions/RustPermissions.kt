package io.idealyst.permissions

import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.util.Log
import io.idealyst.runtime.RustPermissionsResult

/**
 * Launches the Android runtime-permission dialog for the `permissions` SDK
 * and hands the answer back to Rust.
 *
 * Rust calls [request] with the permissions and a request code. This shim
 * registers a handler with the shared [RustPermissionsResult] registry
 * (the generated `MainActivity.onRequestPermissionsResult` forwards to it),
 * then calls `Activity.requestPermissions`. The answer comes back through
 * [nativeOnPermissionsResult], whose JNI export lives in `src/android.rs`.
 */
object RustPermissions {
    private const val TAG = "RustPermissions"

    /** Outcome codes for [nativeOnPermissionsResult]; mirrored in `src/android.rs`. */
    private const val RESULT_GRANTED = 0
    private const val RESULT_DENIED = 1
    private const val RESULT_CANCELLED = 2

    @JvmStatic
    external fun nativeOnPermissionsResult(requestCode: Int, outcome: Int)

    /**
     * Show the dialog for [permissions], any of which will do, under
     * [requestCode]. Returns `false` when the dialog couldn't be launched;
     * nothing is registered then.
     */
    @JvmStatic
    fun request(context: Context, permissions: Array<String>, requestCode: Int): Boolean {
        val activity = context as? Activity
        // An Activity that is gone takes the request and never answers it.
        if (activity == null || activity.isFinishing || activity.isDestroyed) {
            Log.e(TAG, "requesting a permission needs a live Activity")
            return false
        }
        RustPermissionsResult.register(requestCode) { _, grantResults ->
            nativeOnPermissionsResult(requestCode, outcome(grantResults))
            true
        }
        return try {
            activity.requestPermissions(permissions, requestCode)
            true
        } catch (e: Throwable) {
            Log.e(TAG, "requestPermissions failed", e)
            RustPermissionsResult.unregister(requestCode)
            false
        }
    }

    /**
     * Empty results mean the prompt was interrupted, not answered: Android's
     * contract for `onRequestPermissionsResult` says to treat it as a
     * cancellation.
     */
    private fun outcome(grantResults: IntArray): Int = when {
        grantResults.isEmpty() -> RESULT_CANCELLED
        grantResults.any { it == PackageManager.PERMISSION_GRANTED } -> RESULT_GRANTED
        else -> RESULT_DENIED
    }
}
