package io.idealyst.screenrecorder

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.graphics.PixelFormat
import android.hardware.display.DisplayManager
import android.media.ImageReader
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.util.DisplayMetrics
import android.view.PixelCopy
import android.view.WindowManager
import io.idealyst.runtime.RustActivityResult
import java.util.concurrent.ConcurrentHashMap

/**
 * Bridges Android **MediaProjection** + **VirtualDisplay** + **ImageReader**
 * to Rust for the `screen-recorder` SDK. Rust calls [start] with a `token`
 * identifying the awaiting native stream; the shim runs the consent →
 * foreground-service → capture flow and trampolines each captured frame
 * (tightly-packed top-down `RGBA8`) back through [nativeFrame]. Lifecycle
 * (success / failure / declined) goes through [nativeStarted] / [nativeError].
 * [stop] tears the session down.
 *
 * The dance has to live in Kotlin (not raw JNI) because it needs:
 *   1. an Activity-result round-trip for the consent intent (delivered to
 *      `MainActivity.onActivityResult` → [RustActivityResult] → us), and
 *   2. a started foreground service before `getMediaProjection` (a
 *      SecurityException otherwise on API 34).
 *
 * Shipped from the `screen-recorder` SDK crate via
 * `[package.metadata.idealyst.android].runtime_kotlin`; the `native*` symbols
 * are the `#[no_mangle]` exports in `android.rs`.
 */
object RustScreenCaptureHelper {
    // Sentinel `code`s Rust's `map_start_error` recognises. ERR_DENIED maps to
    // RecorderError::PermissionDenied; everything else carries the message.
    private const val ERR_DENIED = -10
    private const val ERR_EXCEPTION = -1

    // Request code for the consent Activity result. Large constant to avoid
    // colliding with any app-level startActivityForResult.
    private const val REQUEST_CONSENT = 0x5C12

    // Source discriminant passed from Rust (`screen_recorder::Source`): 0 =
    // ThisApp → app-only PixelCopy (no consent, no service); anything else →
    // whole-screen MediaProjection.
    private const val SOURCE_THIS_APP = 0

    // ~30fps cadence for the app-only PixelCopy loop.
    private const val PIXELCOPY_INTERVAL_MS = 33L

    private class Session {
        var projection: MediaProjection? = null
        var virtualDisplay: android.hardware.display.VirtualDisplay? = null
        var reader: ImageReader? = null
        var thread: HandlerThread? = null
        var handler: Handler? = null
        // Reusable tight-packed RGBA scratch sized width*height*4; reallocated
        // only when the captured dimensions change.
        var scratch: ByteArray? = null
        // Whether nativeStarted has already fired for this token, so a late
        // error after a successful start doesn't double-resolve the Rust
        // oneshot (which would be a no-op anyway, but keeps intent clear).
        var started = false
        // App-only (Source::ThisApp) PixelCopy path:
        var window: android.view.Window? = null
        var bitmap: android.graphics.Bitmap? = null
        // Cleared on cleanup so the self-reposting capture loop stops.
        var running = false
    }

    private val sessions = ConcurrentHashMap<Long, Session>()

    /**
     * Begin a capture session for [token]. Launches the MediaProjection
     * consent intent; the rest of the flow continues in the Activity-result
     * handler. Must be called with an Activity [context] (the consent intent
     * needs `startActivityForResult`).
     */
    @JvmStatic
    fun start(context: Context, token: Long, source: Int) {
        // Cache the application context for service teardown — cleanup may run
        // after the Activity is gone.
        appContext = context.applicationContext
        // Bounce to the main thread: startActivityForResult + the consent UI
        // (MediaProjection) and reading the Activity window (PixelCopy) must run
        // on the UI thread, and Rust may call us from any thread.
        Handler(context.mainLooper).post {
            try {
                if (source == SOURCE_THIS_APP) {
                    startPixelCopy(context, token)
                } else {
                    launchConsent(context, token)
                }
            } catch (t: Throwable) {
                cleanup(token)
                nativeError(token, ERR_EXCEPTION, t.message ?: t.toString())
            }
        }
    }

    /**
     * App-only capture via [PixelCopy] of the Activity's OWN window — the
     * Android analog of iOS ReplayKit in-app capture. Copying your own window
     * needs no consent dialog and no foreground service, and (because PixelCopy
     * copies one specific window) content rendered on a *separate* window is
     * naturally excluded — the substrate the `PrivateLayer` feature builds on.
     *
     * The loop runs on a private HandlerThread: PixelCopy copies the window's
     * rendered content (including hardware-accelerated layers) into a reused
     * ARGB_8888 Bitmap, whose in-memory byte order is RGBA, so
     * `copyPixelsToBuffer` yields tightly-packed RGBA directly (no rowStride
     * padding to strip, unlike the ImageReader path).
     */
    private fun startPixelCopy(context: Context, token: Long) {
        val activity = context as? Activity
            ?: throw IllegalStateException("app capture requires an Activity context")
        val session = Session()
        session.window = activity.window
        session.running = true
        sessions[token] = session
        val thread = HandlerThread("idealyst-pixelcopy-$token").also { it.start() }
        session.thread = thread
        session.handler = Handler(thread.looper)
        // App capture has no consent gate — the stream is live immediately.
        session.started = true
        nativeStarted(token)
        pixelCopyFrame(token)
    }

    private fun pixelCopyFrame(token: Long) {
        val session = sessions[token] ?: return
        if (!session.running) return
        val window = session.window ?: return
        val handler = session.handler ?: return
        val decor = window.peekDecorView()
        val w = decor?.width ?: 0
        val h = decor?.height ?: 0
        if (w <= 0 || h <= 0) {
            // Window not laid out yet — retry shortly without emitting a frame.
            handler.postDelayed({ pixelCopyFrame(token) }, PIXELCOPY_INTERVAL_MS)
            return
        }
        var bmp = session.bitmap
        if (bmp == null || bmp.width != w || bmp.height != h) {
            bmp?.recycle()
            bmp = android.graphics.Bitmap.createBitmap(
                w, h, android.graphics.Bitmap.Config.ARGB_8888
            )
            session.bitmap = bmp
        }
        val dest = bmp
        try {
            PixelCopy.request(window, dest, { result ->
                val s = sessions[token]
                if (s != null && s.running) {
                    if (result == PixelCopy.SUCCESS) {
                        val buf = java.nio.ByteBuffer.allocate(dest.width * dest.height * 4)
                        dest.copyPixelsToBuffer(buf)
                        nativeFrame(token, buf.array(), dest.width, dest.height)
                    }
                    s.handler?.postDelayed({ pixelCopyFrame(token) }, PIXELCOPY_INTERVAL_MS)
                }
            }, handler)
        } catch (_: Throwable) {
            // PixelCopy can throw if the window's surface isn't ready; retry
            // rather than killing the stream.
            handler.postDelayed({ pixelCopyFrame(token) }, PIXELCOPY_INTERVAL_MS)
        }
    }

    @JvmStatic
    fun stop(token: Long) {
        // Tear down on the main looper: the projection + virtual display were
        // created on the main thread, and Rust calls stop() from the dropping
        // thread (any thread).
        Handler(android.os.Looper.getMainLooper()).post { cleanup(token) }
    }

    private fun launchConsent(context: Context, token: Long) {
        val activity = context as? Activity
            ?: throw IllegalStateException("screen capture requires an Activity context")
        val pm = context.getSystemService(Context.MEDIA_PROJECTION_SERVICE)
            as MediaProjectionManager

        // Register the result handler BEFORE launching, keyed by our request
        // code. The handler is single-shot (RustActivityResult removes it
        // after firing).
        RustActivityResult.register(REQUEST_CONSENT) { resultCode, data ->
            onConsentResult(context, token, resultCode, data)
            true
        }
        activity.startActivityForResult(pm.createScreenCaptureIntent(), REQUEST_CONSENT)
    }

    private fun onConsentResult(context: Context, token: Long, resultCode: Int, data: Intent?) {
        if (resultCode != Activity.RESULT_OK || data == null) {
            cleanup(token)
            nativeError(token, ERR_DENIED, "screen capture consent declined")
            return
        }

        // FGS-before-getMediaProjection: a mediaProjection foreground service
        // must be started AND foregrounded before getMediaProjection, or
        // Android 14 throws SecurityException. We hook the service's
        // "foregrounded" callback to continue, then start it.
        MediaProjectionService.onForegrounded = {
            // Back off the service thread onto a clean main-thread post so we
            // don't do projection setup inside the service's onStartCommand.
            Handler(context.mainLooper).post {
                try {
                    beginCapture(context, token, resultCode, data)
                } catch (t: Throwable) {
                    cleanup(token)
                    nativeError(token, ERR_EXCEPTION, t.message ?: t.toString())
                }
            }
        }
        val svc = Intent(context, MediaProjectionService::class.java)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            context.startForegroundService(svc)
        } else {
            context.startService(svc)
        }
    }

    private fun beginCapture(context: Context, token: Long, resultCode: Int, data: Intent) {
        val pm = context.getSystemService(Context.MEDIA_PROJECTION_SERVICE)
            as MediaProjectionManager
        // Now legal: the FGS is foregrounded.
        val projection = pm.getMediaProjection(resultCode, data)
            ?: throw IllegalStateException("getMediaProjection returned null")

        val session = Session()
        session.projection = projection
        sessions[token] = session

        // Real display metrics. The consent token carries no size, so we mirror
        // the primary display at its native resolution + density.
        val metrics = displayMetrics(context)
        val width = metrics.widthPixels
        val height = metrics.heightPixels
        val densityDpi = metrics.densityDpi

        val thread = HandlerThread("idealyst-screen-capture-$token").also { it.start() }
        val handler = Handler(thread.looper)
        session.thread = thread
        session.handler = handler

        // RGBA_8888 so each plane is a single packed-ish buffer; we strip the
        // rowStride padding below. Two buffers so the producer never stalls.
        val reader = ImageReader.newInstance(width, height, PixelFormat.RGBA_8888, 2)
        session.reader = reader
        reader.setOnImageAvailableListener({ r ->
            val image = r.acquireLatestImage() ?: return@setOnImageAvailableListener
            try {
                val rgba = packRgba(session, image, width, height)
                if (rgba != null) {
                    nativeFrame(token, rgba, width, height)
                }
            } catch (_: Throwable) {
                // Drop the frame; never throw into the listener.
            } finally {
                image.close()
            }
        }, handler)

        // MediaProjection callbacks fire if the user revokes the projection
        // (e.g. via the system "stop sharing" affordance). Tear down + report.
        projection.registerCallback(object : MediaProjection.Callback() {
            override fun onStop() {
                cleanup(token)
            }
        }, handler)

        // AUTO_MIRROR mirrors the default display's content into our surface.
        session.virtualDisplay = projection.createVirtualDisplay(
            "idealyst-capture",
            width,
            height,
            densityDpi,
            DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
            reader.surface,
            null,
            handler,
        )

        session.started = true
        nativeStarted(token)
    }

    /**
     * Copy an `RGBA_8888` [image]'s `planes[0]` into a tightly-packed top-down
     * `RGBA8` buffer of `width*height*4` bytes, honoring the plane's
     * `rowStride`.
     *
     * RGBA_8888 ImageReader rows are padded for alignment: `rowStride` is
     * often larger than `width*4` (and `pixelStride` is 4). Copying the buffer
     * wholesale would interleave padding bytes into the image and shear it, so
     * we copy row-by-row, taking only `width*4` bytes per row and skipping the
     * stride remainder. Returns null on an unexpected plane shape.
     */
    private fun packRgba(session: Session, image: android.media.Image, width: Int, height: Int): ByteArray? {
        val planes = image.planes
        if (planes.isEmpty()) return null
        val plane = planes[0]
        val rowStride = plane.rowStride
        val pixelStride = plane.pixelStride
        if (pixelStride != 4) return null // not packed RGBA8 — bail rather than smear

        val rowBytes = width * 4
        val out = session.scratch?.takeIf { it.size == rowBytes * height }
            ?: ByteArray(rowBytes * height).also { session.scratch = it }

        val buffer = plane.buffer
        if (rowStride == rowBytes) {
            // No padding — a single bulk copy. Clamp to available bytes in
            // case the producer's buffer is exactly tight.
            val n = minOf(out.size, buffer.remaining())
            buffer.get(out, 0, n)
        } else {
            // Padded rows: copy width*4 bytes per row, skipping the padding.
            var pos = 0
            for (row in 0 until height) {
                buffer.position(row * rowStride)
                buffer.get(out, pos, rowBytes)
                pos += rowBytes
            }
        }
        return out
    }

    /** Primary-display metrics (pixels + densityDpi). */
    private fun displayMetrics(context: Context): DisplayMetrics {
        val metrics = DisplayMetrics()
        val wm = context.getSystemService(Context.WINDOW_SERVICE) as WindowManager
        @Suppress("DEPRECATION")
        wm.defaultDisplay.getRealMetrics(metrics)
        return metrics
    }

    /** Tear down everything associated with [token]. Idempotent. */
    private fun cleanup(token: Long) {
        // Clear the service hook so a stale callback can't fire into a torn-down
        // session.
        MediaProjectionService.onForegrounded = {}
        RustActivityResult.unregister(REQUEST_CONSENT)
        val session = sessions.remove(token) ?: run {
            // Even with no session, stop the FGS in case consent was declined
            // after the service started (it normally isn't started until OK).
            stopService(token)
            return
        }
        // App-only PixelCopy path: stop the self-reposting loop + free the bitmap.
        session.running = false
        try {
            session.bitmap?.recycle()
        } catch (_: Throwable) {
        }
        try {
            session.virtualDisplay?.release()
        } catch (_: Throwable) {
        }
        try {
            session.reader?.close()
        } catch (_: Throwable) {
        }
        try {
            session.projection?.stop()
        } catch (_: Throwable) {
        }
        session.thread?.quitSafely()
        stopService(token)
    }

    /** Stop the mediaProjection foreground service. */
    private fun stopService(token: Long) {
        try {
            val app = appContext ?: return
            app.stopService(Intent(app, MediaProjectionService::class.java))
        } catch (_: Throwable) {
        }
    }

    // Cached application context for service teardown (cleanup may run without
    // an Activity handy). Set on the first start().
    @Volatile
    private var appContext: Context? = null

    @JvmStatic
    private external fun nativeStarted(token: Long)

    @JvmStatic
    private external fun nativeError(token: Long, code: Int, message: String?)

    @JvmStatic
    private external fun nativeFrame(token: Long, data: ByteArray, width: Int, height: Int)
}
