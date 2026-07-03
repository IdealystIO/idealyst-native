package io.idealyst.fileexport

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.util.Log
import io.idealyst.runtime.RustActivityResult
import java.util.concurrent.ConcurrentHashMap

/**
 * Drives the Android **Storage Access Framework** for the `file-export` SDK.
 * Rust calls [save] with the bytes + a suggested name + MIME type; this shim
 * launches `ACTION_CREATE_DOCUMENT` (the system document creator), and when
 * the user picks a destination writes the bytes to that `content://` URI via
 * the `ContentResolver`, then trampolines the outcome back to the `native*`
 * exports in `android.rs`.
 *
 * The Activity-result round-trip is routed through the shared
 * [RustActivityResult] registry (the host `MainActivity.onActivityResult`
 * forwards to it), so no `MainActivity` edits are needed — the same path
 * `screen-recorder`'s consent intent uses.
 *
 * VERIFICATION: compile-checked against the SDK's JNI signatures; the SAF /
 * ContentResolver path resolves only at runtime on a device.
 */
object RustFileExport {
    private const val TAG = "RustFileExport"

    // Request code for our CREATE_DOCUMENT result. Large constant to avoid
    // colliding with app-level startActivityForResult (screen-capture uses
    // 0x5C12; this is a distinct value).
    private const val REQUEST_SAVE = 0x5A1E

    // token → bytes to write once the user picks a destination. Held only
    // between launch and the single-shot result.
    private val pending = ConcurrentHashMap<Long, ByteArray>()

    @JvmStatic
    external fun nativeSaved(token: Long, location: String?)

    @JvmStatic
    external fun nativeCancelled(token: Long)

    @JvmStatic
    external fun nativeError(token: Long, message: String?)

    @JvmStatic
    fun save(context: Context, token: Long, suggestedName: String, mime: String, bytes: ByteArray) {
        try {
            val activity = context as? Activity
                ?: throw IllegalStateException("file export requires an Activity context")
            pending[token] = bytes

            RustActivityResult.register(REQUEST_SAVE) { resultCode, data ->
                onResult(context, token, resultCode, data)
                true
            }

            val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = mime
                putExtra(Intent.EXTRA_TITLE, suggestedName)
            }
            activity.startActivityForResult(intent, REQUEST_SAVE)
        } catch (e: Throwable) {
            Log.e(TAG, "save launch failed", e)
            pending.remove(token)
            nativeError(token, e.message ?: e.toString())
        }
    }

    private fun onResult(context: Context, token: Long, resultCode: Int, data: Intent?) {
        val bytes = pending.remove(token)
        if (bytes == null) {
            nativeError(token, "no pending bytes for save")
            return
        }
        val uri = data?.data
        if (resultCode != Activity.RESULT_OK || uri == null) {
            nativeCancelled(token)
            return
        }
        try {
            context.contentResolver.openOutputStream(uri)?.use { out ->
                out.write(bytes)
                out.flush()
            } ?: throw IllegalStateException("could not open output stream for $uri")
            nativeSaved(token, uri.toString())
        } catch (e: Throwable) {
            Log.e(TAG, "write failed", e)
            nativeError(token, e.message ?: e.toString())
        }
    }
}
