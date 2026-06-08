package io.idealyst.filepicker

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.util.Log
import io.idealyst.runtime.RustActivityResult

/**
 * Drives Android file picking for the `file-picker` SDK. Rust calls [pick] with
 * the MIME filters + flags; this shim launches the right intent:
 *
 *  - documents → `ACTION_OPEN_DOCUMENT` (the system file browser), and
 *  - media → the **Photo Picker** (`ACTION_PICK_IMAGES`) on API 33+, falling
 *    back to `ACTION_OPEN_DOCUMENT` filtered to image/video on older devices.
 *
 * When the user picks, it queries each `content://` URI's display name, size,
 * and type and trampolines them back to the `native*` exports in `android.rs`.
 * The bytes themselves never cross JNI: Rust later calls [openFd], which detaches
 * a read file descriptor for streaming.
 *
 * The Activity-result round-trip is routed through the shared
 * [RustActivityResult] registry (the host `MainActivity.onActivityResult`
 * forwards to it), so no `MainActivity` edits are needed — the same path
 * `file-export`'s SAF save uses.
 *
 * VERIFICATION: compile-checked against the SDK's JNI signatures; the
 * ContentResolver / Photo Picker path resolves only at runtime on a device.
 */
object RustFilePicker {
    private const val TAG = "RustFilePicker"

    // Request code for our pick result. Distinct from file-export's 0x5A1E.
    private const val REQUEST_PICK = 0x5A1F

    @JvmStatic
    external fun nativeFilesPicked(
        token: Long,
        count: Int,
        uris: Array<String>,
        names: Array<String>,
        mimes: Array<String>,
        sizes: LongArray,
    )

    @JvmStatic
    external fun nativeCancelled(token: Long)

    @JvmStatic
    external fun nativeError(token: Long, message: String?)

    @JvmStatic
    fun pick(
        context: Context,
        token: Long,
        mimes: Array<String>,
        allowMultiple: Boolean,
        isMedia: Boolean,
    ) {
        try {
            val activity = context as? Activity
                ?: throw IllegalStateException("file picking requires an Activity context")

            RustActivityResult.register(REQUEST_PICK) { resultCode, data ->
                onResult(context, token, resultCode, data)
                true
            }

            val intent = if (isMedia && Build.VERSION.SDK_INT >= 33) {
                mediaPickerIntent(mimes, allowMultiple)
            } else {
                openDocumentIntent(mimes, allowMultiple, isMedia)
            }
            activity.startActivityForResult(intent, REQUEST_PICK)
        } catch (e: Throwable) {
            Log.e(TAG, "pick launch failed", e)
            nativeError(token, e.message ?: e.toString())
        }
    }

    /** The Android Photo Picker (`ACTION_PICK_IMAGES`), API 33+. */
    private fun mediaPickerIntent(mimes: Array<String>, allowMultiple: Boolean): Intent {
        val intent = Intent(MediaStore.ACTION_PICK_IMAGES)
        // One wildcard ("image/*" or "video/*") narrows the picker; both (or
        // none) leaves it on photos + videos.
        if (mimes.size == 1) intent.type = mimes[0]
        if (allowMultiple) {
            // ACTION_PICK_IMAGES requires an explicit max for multi-select.
            intent.putExtra(MediaStore.EXTRA_PICK_IMAGES_MAX, 100)
        }
        return intent
    }

    /** The system document browser (`ACTION_OPEN_DOCUMENT`). */
    private fun openDocumentIntent(
        mimes: Array<String>,
        allowMultiple: Boolean,
        isMedia: Boolean,
    ): Intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
        addCategory(Intent.CATEGORY_OPENABLE)
        val filters = if (isMedia && mimes.isEmpty()) arrayOf("image/*", "video/*") else mimes
        type = if (filters.size == 1) filters[0] else "*/*"
        if (filters.isNotEmpty()) putExtra(Intent.EXTRA_MIME_TYPES, filters)
        putExtra(Intent.EXTRA_ALLOW_MULTIPLE, allowMultiple)
    }

    private fun onResult(context: Context, token: Long, resultCode: Int, data: Intent?) {
        if (resultCode != Activity.RESULT_OK || data == null) {
            nativeCancelled(token)
            return
        }
        // Multi-select arrives as clipData; single as data.data.
        val uris = ArrayList<Uri>()
        val clip = data.clipData
        if (clip != null) {
            for (i in 0 until clip.itemCount) uris.add(clip.getItemAt(i).uri)
        } else {
            data.data?.let { uris.add(it) }
        }
        if (uris.isEmpty()) {
            nativeCancelled(token)
            return
        }

        try {
            val resolver = context.contentResolver
            val n = uris.size
            val uriStrs = Array(n) { "" }
            val names = Array(n) { "" }
            val mimes = Array(n) { "" }
            val sizes = LongArray(n) { -1L }
            for ((i, uri) in uris.withIndex()) {
                uriStrs[i] = uri.toString()
                mimes[i] = resolver.getType(uri) ?: ""
                resolver.query(uri, null, null, null, null)?.use { c ->
                    if (c.moveToFirst()) {
                        val ni = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                        if (ni >= 0) names[i] = c.getString(ni) ?: ""
                        val si = c.getColumnIndex(OpenableColumns.SIZE)
                        if (si >= 0 && !c.isNull(si)) sizes[i] = c.getLong(si)
                    }
                }
            }
            nativeFilesPicked(token, n, uriStrs, names, mimes, sizes)
        } catch (e: Throwable) {
            Log.e(TAG, "reading pick result failed", e)
            nativeError(token, e.message ?: e.toString())
        }
    }

    /**
     * Open a read file descriptor for a picked URI and **detach** it, handing
     * ownership to native code (Rust closes it on drop). Returns -1 on failure.
     *
     * Detaching (not just returning the fd) is deliberate: the
     * `ParcelFileDescriptor` must not close the fd when it's GC'd, or Rust would
     * read a closed descriptor.
     */
    @JvmStatic
    fun openFd(context: Context, uri: String): Int {
        return try {
            val pfd = context.contentResolver.openFileDescriptor(Uri.parse(uri), "r")
                ?: return -1
            pfd.detachFd()
        } catch (e: Throwable) {
            Log.e(TAG, "openFd failed for $uri", e)
            -1
        }
    }
}
