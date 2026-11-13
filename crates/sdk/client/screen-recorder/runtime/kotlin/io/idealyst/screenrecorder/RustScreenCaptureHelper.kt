package io.idealyst.screenrecorder

import android.app.Activity
import android.app.AppOpsManager
import android.content.Context
import android.content.Intent
import android.graphics.PixelFormat
import android.hardware.display.DisplayManager
import android.media.ImageReader
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionConfig
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.util.DisplayMetrics
import android.view.PixelCopy
import android.view.Surface
import android.view.WindowManager
import java.nio.ByteBuffer
import java.util.concurrent.ConcurrentHashMap

/**
 * Bridges Android **MediaProjection** + **VirtualDisplay** to Rust for the
 * `screen-recorder` SDK. Rust calls [start] with a `token` identifying the
 * awaiting native stream; the shim runs the consent → foreground-service →
 * capture flow, and lifecycle (success / failure / declined) goes through
 * [nativeStarted] / [nativeError]. [stop] tears the session down.
 *
 * The virtual display has one surface at a time, and which one follows
 * demand:
 *   - a consumer's own surface while one is attached ([attachSurface], from
 *     the stream's `PreviewSource`: an encoder's input surface, a display's
 *     `TextureView`). Frames go GPU to consumer and never reach this process.
 *   - otherwise an `RGBA_8888` [ImageReader] while a CPU subscriber wants
 *     frames ([setCpuDemand]); each is packed tight and handed over through
 *     [nativeFrameDirect].
 *   - otherwise none: nothing is rendered.
 * An attached consumer wins over CPU subscribers, who get no frames until it
 * detaches. A projection allows one virtual display on Android 14+, so the
 * surface is switched on that one display rather than a second one made.
 *
 * Consent goes through [ScreenCaptureConsentActivity], a see-through activity
 * in a task of its own, not the app's activity: an app capturing from behind
 * another (a device-owner kiosk agent) would otherwise bring its own task
 * forward to ask.
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

    // Source discriminant passed from Rust (`screen_recorder::Source`): 0 =
    // ThisApp → app-only PixelCopy (no consent, no service); anything else →
    // whole-screen MediaProjection.
    private const val SOURCE_THIS_APP = 0

    // ~30fps cadence for the app-only PixelCopy loop.
    private const val PIXELCOPY_INTERVAL_MS = 33L

    /** What [ScreenCaptureConsentActivity] needs to carry on once it has an answer. */
    private class Request(val width: Int, val height: Int)

    private val requests = ConcurrentHashMap<Long, Request>()

    private val main by lazy { Handler(Looper.getMainLooper()) }

    private class Session {
        var projection: MediaProjection? = null
        var virtualDisplay: android.hardware.display.VirtualDisplay? = null
        var reader: ImageReader? = null
        var token = 0L
        var width = 0
        var height = 0
        // A consumer's surface, while one is attached.
        var attached: Surface? = null
        var cpuDemand = false
        var thread: HandlerThread? = null
        var handler: Handler? = null
        // Reusable DIRECT RGBA output buffer sized width*height*4; reused
        // across frames so a steady capture allocates nothing per frame, and
        // Rust reads it zero-copy via GetDirectBufferAddress (no byte[] marshal,
        // no Rust Vec). Re-allocated only when the captured dimensions change.
        var directBuf: ByteBuffer? = null
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
     * Begin a capture session for [token]. A screen source asks for consent
     * through [ScreenCaptureConsentActivity] and carries on from its answer;
     * frames come out at [width] × [height] (Rust sizes them from
     * [displaySize] and the request). The app-only source needs an Activity
     * [context], whose window it copies.
     */
    @JvmStatic
    fun start(context: Context, token: Long, source: Int, width: Int, height: Int) {
        // Cache the application context for service teardown — cleanup may run
        // after the Activity is gone.
        appContext = context.applicationContext
        // Bounce to the main thread: starting the consent activity and reading
        // the Activity window (PixelCopy) must run on the UI thread, and Rust
        // may call us from any thread.
        main.post {
            try {
                if (source == SOURCE_THIS_APP) {
                    startPixelCopy(context, token)
                } else {
                    requests[token] = Request(width, height)
                    val app = context.applicationContext
                    app.startActivity(
                        Intent(app, ScreenCaptureConsentActivity::class.java)
                            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION)
                            .putExtra(ScreenCaptureConsentActivity.EXTRA_TOKEN, token),
                    )
                }
            } catch (t: Throwable) {
                requests.remove(token)
                cleanup(token)
                nativeError(token, ERR_EXCEPTION, t.message ?: t.toString())
            }
        }
    }

    /** The default display's real size in pixels, packed `width << 16 | height`. */
    @JvmStatic
    fun displaySize(context: Context): Int {
        val m = displayMetrics(context)
        return (m.widthPixels shl 16) or (m.heightPixels and 0xffff)
    }

    /**
     * Whether consent is already given (the `PROJECT_MEDIA` app op, which only
     * adb or the system can grant), so [start] shows nobody a prompt.
     */
    @JvmStatic
    fun consentPregranted(context: Context): Boolean {
        val ops = context.getSystemService(Context.APP_OPS_SERVICE) as AppOpsManager
        @Suppress("DEPRECATION")
        val mode = ops.unsafeCheckOpNoThrow("android:project_media", android.os.Process.myUid(), context.packageName)
        return mode == AppOpsManager.MODE_ALLOWED
    }

    /** Render into [surface] (a consumer's), in place of anything else, until [detachSurface]. */
    @JvmStatic
    fun attachSurface(token: Long, surface: Surface) {
        main.post {
            val s = sessions[token] ?: return@post
            s.attached = surface
            route(s)
        }
    }

    /** Stop rendering into the attached surface, which its consumer is about to release. */
    @JvmStatic
    fun detachSurface(token: Long) {
        main.post {
            val s = sessions[token] ?: return@post
            s.attached = null
            route(s)
        }
    }

    /** Whether CPU subscribers want frames; an [ImageReader] exists only while they do. */
    @JvmStatic
    fun setCpuDemand(token: Long, wants: Boolean) {
        main.post {
            val s = sessions[token] ?: return@post
            s.cpuDemand = wants
            route(s)
        }
    }

    /** Point the virtual display at what wants frames now (see the class docs). */
    private fun route(s: Session) {
        val display = s.virtualDisplay ?: return
        display.surface = s.attached ?: if (s.cpuDemand) readerFor(s).surface else null
        // Closed only once the display has stopped rendering into it.
        if (!s.cpuDemand) {
            s.reader?.close()
            s.reader = null
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
        // No virtual display, so no size to report and no surface to attach.
        nativeStarted(token, 0, 0)
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
                        val needed = dest.width * dest.height * 4
                        var buf = s.directBuf
                        if (buf == null || buf.capacity() != needed) {
                            buf = ByteBuffer.allocateDirect(needed)
                            s.directBuf = buf
                        }
                        buf.clear()
                        dest.copyPixelsToBuffer(buf)
                        nativeFrameDirect(token, buf, dest.width, dest.height)
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

    /** The whole default display, never one app: on 14+ the prompt would otherwise offer a choice. */
    internal fun consentIntent(context: Context): Intent {
        val pm = context.getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
        return if (Build.VERSION.SDK_INT >= 34) {
            pm.createScreenCaptureIntent(MediaProjectionConfig.createConfigForDefaultDisplay())
        } else {
            pm.createScreenCaptureIntent()
        }
    }

    /**
     * [ScreenCaptureConsentActivity]'s answer, while it is still in front: that
     * is what lets the foreground service start from an app in the background.
     */
    internal fun onConsentResult(context: Context, token: Long, resultCode: Int, data: Intent?) {
        val request = requests.remove(token) ?: return
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
                    beginCapture(context, token, request, resultCode, data)
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

    private fun beginCapture(context: Context, token: Long, request: Request, resultCode: Int, data: Intent) {
        val pm = context.getSystemService(Context.MEDIA_PROJECTION_SERVICE)
            as MediaProjectionManager
        // Now legal: the FGS is foregrounded.
        val projection = pm.getMediaProjection(resultCode, data)
            ?: throw IllegalStateException("getMediaProjection returned null")

        val session = Session()
        session.projection = projection
        session.token = token
        session.width = request.width
        session.height = request.height
        sessions[token] = session

        val thread = HandlerThread("idealyst-screen-capture-$token").also { it.start() }
        val handler = Handler(thread.looper)
        session.thread = thread
        session.handler = handler

        // Registered before the virtual display: Android 14+ refuses one
        // otherwise. Fires if the projection is revoked (the system's "stop
        // sharing").
        projection.registerCallback(object : MediaProjection.Callback() {
            override fun onStop() {
                main.post { cleanup(token) }
            }
        }, handler)

        // AUTO_MIRROR mirrors the default display, scaled into whatever surface
        // [route] gives it; none yet, so nothing is rendered until something
        // wants frames.
        session.virtualDisplay = projection.createVirtualDisplay(
            "idealyst-capture",
            request.width,
            request.height,
            displayMetrics(context).densityDpi,
            DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
            null,
            null,
            handler,
        )
        route(session)

        session.started = true
        nativeStarted(token, request.width, request.height)
    }

    /** The session's CPU reader, made on first use at the capture size. */
    private fun readerFor(session: Session): ImageReader {
        session.reader?.let { return it }
        val width = session.width
        val height = session.height
        // Two buffers so the producer never stalls.
        val reader = ImageReader.newInstance(width, height, PixelFormat.RGBA_8888, 2)
        reader.setOnImageAvailableListener({ r ->
            val image = r.acquireLatestImage() ?: return@setOnImageAvailableListener
            try {
                val needed = width * height * 4
                var buf = session.directBuf
                if (buf == null || buf.capacity() != needed) {
                    buf = ByteBuffer.allocateDirect(needed)
                    session.directBuf = buf
                }
                if (packRgba(image, width, height, buf)) {
                    nativeFrameDirect(session.token, buf, width, height)
                }
            } catch (_: Throwable) {
                // Drop the frame; never throw into the listener.
            } finally {
                image.close()
            }
        }, session.handler)
        session.reader = reader
        return reader
    }

    /**
     * Copy an `RGBA_8888` [image]'s `planes[0]` into [out] (a cleared direct
     * ByteBuffer of `width*height*4` bytes) as tightly-packed top-down `RGBA8`,
     * honoring the plane's `rowStride`. Returns false on an unexpected plane
     * shape (caller skips the frame).
     *
     * RGBA_8888 ImageReader rows are padded for alignment: `rowStride` is
     * often larger than `width*4` (and `pixelStride` is 4). Copying the buffer
     * wholesale would interleave padding bytes into the image and shear it, so
     * we copy row-by-row, taking only `width*4` bytes per row and skipping the
     * stride remainder. The plane buffer is mutated (position/limit) per row,
     * which is fine — it's the Image's own buffer, consumed once before close.
     */
    private fun packRgba(image: android.media.Image, width: Int, height: Int, out: ByteBuffer): Boolean {
        val planes = image.planes
        if (planes.isEmpty()) return false
        val plane = planes[0]
        val rowStride = plane.rowStride
        val pixelStride = plane.pixelStride
        if (pixelStride != 4) return false // not packed RGBA8 — bail rather than smear

        val rowBytes = width * 4
        val buffer = plane.buffer
        out.clear()
        if (rowStride == rowBytes) {
            // No padding — a single bulk copy. Clamp to available bytes in
            // case the producer's buffer is exactly tight.
            buffer.clear()
            buffer.limit(minOf(rowBytes * height, buffer.capacity()))
            out.put(buffer)
        } else {
            // Padded rows: copy width*4 bytes per row, skipping the padding.
            for (row in 0 until height) {
                val start = row * rowStride
                buffer.clear()
                buffer.position(start)
                buffer.limit(start + rowBytes)
                out.put(buffer)
            }
        }
        return true
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
        requests.remove(token)
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
    private external fun nativeStarted(token: Long, width: Int, height: Int)

    @JvmStatic
    private external fun nativeError(token: Long, code: Int, message: String?)

    @JvmStatic
    private external fun nativeFrameDirect(
        token: Long,
        buffer: ByteBuffer,
        width: Int,
        height: Int,
    )
}
