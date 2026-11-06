package io.idealyst.camera

import android.content.Context
import android.graphics.ImageFormat
import android.graphics.Point
import android.graphics.SurfaceTexture
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.display.DisplayManager
import android.media.Image
import android.media.ImageReader
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.util.Log
import android.util.Range
import android.util.Size
import android.view.Display
import android.view.Surface
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicReference

/**
 * Bridges Android **Camera2** to Rust for the `camera` SDK. Rust calls [open] with
 * a `token` identifying the awaiting native stream; the shim opens the camera and
 * runs one capture session over two outputs: a display's `Surface` ([attachPreview])
 * and an `ImageReader`. The repeating request moves between them on demand
 * ([setCpuDemand]), so a displayed preview reaches the screen with no copy into
 * this process and `YUV_420_888` frames are converted to `RGBA8` (in Rust, through
 * [nativeFrameYuv]) only while a consumer wants them. Lifecycle (success / failure)
 * goes through [nativeOpened] / [nativeError]. [close] tears the session down.
 *
 * **Getting a surface into a session.** A session's outputs are fixed at creation,
 * and a display's `Surface` only exists after its first layout, which is after the
 * stream exists. On API 26+ the first session declares a deferred preview output
 * (`OutputConfiguration(Size, SurfaceTexture)`), and the surface fills it through
 * `finalizeOutputConfigurations` with the session running, so the display's arrival
 * interrupts nothing and a CPU consumer already reading sees no gap. A deferred
 * output fills once. Any later surface, and every surface below API 26 or on a
 * LEGACY device (whose shim has no deferred outputs), rebuilds the session, which
 * costs a pause of a few frames. Rebuilding is also the fallback if anything about a
 * deferred output is refused.
 *
 * **Refusal.** Camera2 guarantees a preview plus a YUV reader at `PREVIEW` size on
 * every hardware level, but a session can still fail on a real device. If a session
 * holding a surface fails, the shim rebuilds with the reader alone and reports
 * [nativePreviewRefused], and displays fall back to CPU frames rather than black.
 *
 * **A slow reader must not slow the preview.** Every target of a request has to get
 * its buffer before the request runs, so a reader short of free images stalls the
 * preview too. The reader's listener therefore never does the conversion: it takes
 * the newest image, replaces whichever is waiting, and leaves the work to a second
 * thread, so the capture pipeline always finds a free image.
 *
 * **Threads.** Session state is touched on the capture thread (`session.handler`),
 * and every entry point posts there. [close] runs on the main thread so a close
 * followed by an open reaches Camera2 in that order.
 *
 * Camera2 is callback-driven and the open must be issued on a looper thread,
 * which is why this lives in Kotlin rather than raw JNI. Shipped from the
 * `camera` SDK crate via `[package.metadata.idealyst.android].runtime_kotlin`;
 * the `native*` symbols are the `#[no_mangle]` exports in `android.rs`.
 */
object RustCamera2Helper {
    // Sentinels Rust's `map_open_error` recognises.
    private const val ERR_NO_CAMERA = -2
    private const val ERR_UNSUPPORTED_CONFIG = -3
    private const val ERR_EXCEPTION = -1

    // CameraConfig::facing as passed from Rust.
    private const val FACING_DEFAULT = 0
    private const val FACING_FRONT = 1
    private const val FACING_BACK = 2

    private const val TAG = "idealyst-camera"

    // Bits of `nativeRepeatingTargets`, from `android_preview.rs`.
    private const val TARGET_PREVIEW = 1
    private const val TARGET_READER = 2

    private class Session {
        var device: CameraDevice? = null
        var captureSession: CameraCaptureSession? = null
        var reader: ImageReader? = null
        var thread: HandlerThread? = null
        var handler: Handler? = null
        // Runs the YUV conversion, so the reader's listener never waits on it.
        var convertThread: HandlerThread? = null
        var convertHandler: Handler? = null
        // The newest image waiting for the conversion thread. Holding one here and
        // one mid-conversion, with one acquired by the listener in between, is why
        // the reader has three images.
        val pendingImage = AtomicReference<Image?>(null)
        var fps: Int = 0
        // The size a display's surface takes, or null when this device has none for
        // this reader's frames.
        var previewSize: Size? = null
        // May the next session without a surface declare a deferred preview output?
        var mayDefer: Boolean = false
        // The current session's deferred output, until a surface fills it.
        var deferredOutput: OutputConfiguration? = null
        // The surface a display has attached.
        var previewSurface: Surface? = null
        // The surface that is an output of the current session. A preview is live
        // when these are the same one.
        var sessionSurface: Surface? = null
        // A session is being created; requests wait for onConfigured.
        var configuring: Boolean = false
        var opened: Boolean = false
        // Written by [setCpuDemand] from any thread, read per image.
        @Volatile var cpuWanted: Boolean = false
        @Volatile var closed: Boolean = false
        // SENSOR_ORIENTATION (0/90/180/270): how the sensor is mounted
        // relative to the device's natural orientation.
        var sensorOrientation: Int = 0
        // LENS_FACING == FRONT. The front camera turns the opposite way on a
        // display rotation (it faces the user).
        var front: Boolean = false
        // Clockwise degrees each frame is rotated by so it's upright on the
        // CURRENT display rotation — `nativeFrameRotation(sensor, display,
        // front)`, the host-tested Rust formula. Written on the main thread
        // (open + every display rotation), read per frame on the capture
        // thread, hence @Volatile. Up to camera 1.6.0 this was the sensor
        // angle alone, so frames were only upright with the device held
        // naturally (portrait) — 90°/180° off in every other orientation.
        @Volatile var rotation: Int = 0
        // Re-computes `rotation` when the display rotates; unregistered in
        // cleanup().
        var displayManager: DisplayManager? = null
        var displayListener: DisplayManager.DisplayListener? = null
    }

    private val sessions = ConcurrentHashMap<Long, Session>()

    /**
     * Every camera, flattened as `[id0, lensFacing0, id1, lensFacing1, …]`
     * where `lensFacing` is `CameraCharacteristics.LENS_FACING` as a decimal
     * string (`-1` when unreported). Rust's `devices()` builds the list from
     * it. Needs no permission and no looper, so it runs on the caller's thread.
     */
    @JvmStatic
    fun listCameras(context: Context): Array<String> {
        val manager = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
        val out = ArrayList<String>()
        for (id in manager.cameraIdList) {
            val lens = manager.getCameraCharacteristics(id).get(CameraCharacteristics.LENS_FACING)
            out.add(id)
            out.add((lens ?: -1).toString())
        }
        return out.toTypedArray()
    }

    /**
     * Open [cameraId] when non-null (an id from [listCameras]); otherwise the
     * camera [facing] resolves to.
     */
    @JvmStatic
    fun open(
        context: Context,
        cameraId: String?,
        facing: Int,
        width: Int,
        height: Int,
        fps: Int,
        token: Long,
    ) {
        Handler(Looper.getMainLooper()).post {
            try {
                start(context, cameraId, facing, width, height, fps, token)
            } catch (t: Throwable) {
                cleanup(token)
                nativeError(token, ERR_EXCEPTION, t.message ?: t.toString())
            }
        }
    }

    @JvmStatic
    fun close(token: Long) {
        Handler(Looper.getMainLooper()).post { cleanup(token) }
    }

    private fun start(
        context: Context,
        requestedId: String?,
        facing: Int,
        width: Int,
        height: Int,
        fps: Int,
        token: Long,
    ) {
        val manager = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
        val cameraId = if (requestedId != null) {
            requestedId.takeIf { it in manager.cameraIdList }
        } else {
            pickCameraId(manager, facing)
        }
        if (cameraId == null) {
            nativeError(token, ERR_NO_CAMERA, "no camera ${requestedId ?: "matching facing=$facing"}")
            return
        }

        val characteristics = manager.getCameraCharacteristics(cameraId)
        val configMap =
            characteristics.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
        val sizes = configMap?.getOutputSizes(ImageFormat.YUV_420_888)
        if (sizes == null || sizes.isEmpty()) {
            nativeError(token, ERR_UNSUPPORTED_CONFIG, "no YUV_420_888 output sizes")
            return
        }
        val size = chooseSize(sizes, width, height)
        if (size == null) {
            nativeError(token, ERR_UNSUPPORTED_CONFIG, "no ${width}x${height} capture size")
            return
        }

        val session = Session()
        session.fps = fps
        session.sensorOrientation =
            characteristics.get(CameraCharacteristics.SENSOR_ORIENTATION) ?: 0
        session.front = characteristics.get(CameraCharacteristics.LENS_FACING) ==
            CameraCharacteristics.LENS_FACING_FRONT
        session.previewSize = choosePreviewSize(context, configMap, size)
        session.mayDefer = Build.VERSION.SDK_INT >= 26 && session.previewSize != null &&
            characteristics.get(CameraCharacteristics.INFO_SUPPORTED_HARDWARE_LEVEL) !=
            CameraCharacteristics.INFO_SUPPORTED_HARDWARE_LEVEL_LEGACY
        sessions[token] = session

        val thread = HandlerThread("idealyst-camera-$token").also { it.start() }
        val handler = Handler(thread.looper)
        session.thread = thread
        session.handler = handler
        val convertThread = HandlerThread("idealyst-camera-convert-$token").also { it.start() }
        val convertHandler = Handler(convertThread.looper)
        session.convertThread = convertThread
        session.convertHandler = convertHandler

        followDisplayRotation(context, session)

        val reader = ImageReader.newInstance(size.width, size.height, ImageFormat.YUV_420_888, 3)
        session.reader = reader
        val convertWaiting = Runnable { convertWaitingImage(session, token) }
        reader.setOnImageAvailableListener({ r ->
            val image = r.acquireLatestImage() ?: return@setOnImageAvailableListener
            // Frames nobody wants: the reader is a target only to keep the sensor
            // running with no display attached, or for the few frames still in
            // flight after demand fell.
            if (!session.cpuWanted) {
                image.close()
                return@setOnImageAvailableListener
            }
            val replaced = session.pendingImage.getAndSet(image)
            if (replaced != null) {
                // The conversion is behind and already has a run queued for
                // whichever image is waiting. Newest wins.
                replaced.close()
            } else {
                convertHandler.post(convertWaiting)
            }
        }, handler)

        // openCamera requires CAMERA permission (checked on the Rust side);
        // SecurityException here would be surfaced via the catch in open().
        manager.openCamera(cameraId, object : CameraDevice.StateCallback() {
            override fun onOpened(device: CameraDevice) {
                if (session.closed) {
                    device.close()
                    return
                }
                session.device = device
                configureSession(session, token)
            }

            override fun onDisconnected(device: CameraDevice) {
                cleanup(token)
                nativeError(token, ERR_EXCEPTION, "camera disconnected")
            }

            override fun onError(device: CameraDevice, error: Int) {
                cleanup(token)
                nativeError(token, error, "camera device error")
            }
        }, handler)
    }

    /**
     * The conversion thread's one job: take the waiting image, if any, and convert
     * it. Rust reads the planes for exactly the length of the call, and they are
     * valid until `image.close()`. One read of the rotation per frame: a rotation
     * landing mid-frame applies from the next frame, never half-way through this one.
     */
    private fun convertWaitingImage(session: Session, token: Long) {
        val image = session.pendingImage.getAndSet(null) ?: return
        try {
            if (session.cpuWanted) {
                val planes = image.planes
                nativeFrameYuv(
                    token,
                    planes[0].buffer, planes[0].rowStride, planes[0].pixelStride,
                    planes[1].buffer, planes[1].rowStride, planes[1].pixelStride,
                    planes[2].buffer, planes[2].rowStride, planes[2].pixelStride,
                    image.width,
                    image.height,
                    session.rotation,
                )
            }
        } catch (_: Throwable) {
            // Drop the frame; never throw into the looper.
        } finally {
            image.close()
        }
    }

    /**
     * Create a capture session for the current state, replacing any running one.
     * Holds the attached surface if there is one; otherwise a deferred output if
     * that is still possible; otherwise the reader alone. Runs on the capture thread.
     */
    private fun configureSession(session: Session, token: Long) {
        val device = session.device ?: return
        val readerSurface = session.reader?.surface ?: return
        val surface = session.previewSurface
        val previewSize = session.previewSize
        val deferred =
            if (surface == null && session.mayDefer && previewSize != null) {
                OutputConfiguration(previewSize, SurfaceTexture::class.java)
            } else {
                null
            }
        session.mayDefer = false
        session.configuring = true
        session.captureSession = null
        session.deferredOutput = null
        session.sessionSurface = null
        val callback = object : CameraCaptureSession.StateCallback() {
            override fun onConfigured(captureSession: CameraCaptureSession) {
                session.configuring = false
                if (session.closed) {
                    captureSession.close()
                    return
                }
                session.captureSession = captureSession
                session.deferredOutput = deferred
                session.sessionSurface = surface
                Log.i(TAG, "session configured: " + describeOutputs(surface, deferred, previewSize))
                try {
                    // A surface may have attached while this session was being made.
                    linkPreview(session, token)
                    if (!session.opened) {
                        session.opened = true
                        nativeOpened(
                            token,
                            previewSize?.width ?: 0,
                            previewSize?.height ?: 0,
                            session.sensorOrientation,
                            session.front,
                        )
                    }
                } catch (t: Throwable) {
                    cleanup(token)
                    nativeError(token, ERR_EXCEPTION, t.message ?: t.toString())
                }
            }

            override fun onConfigureFailed(captureSession: CameraCaptureSession) {
                sessionFailed(session, token, surface, deferred != null, "configuration failed")
            }
        }
        try {
            when {
                surface != null ->
                    device.createCaptureSession(
                        listOf(surface, readerSurface),
                        callback,
                        session.handler,
                    )
                deferred != null ->
                    device.createCaptureSessionByOutputConfigurations(
                        listOf(deferred, OutputConfiguration(readerSurface)),
                        callback,
                        session.handler,
                    )
                else ->
                    device.createCaptureSession(listOf(readerSurface), callback, session.handler)
            }
        } catch (t: Throwable) {
            sessionFailed(session, token, surface, deferred != null, t.toString())
        }
    }

    /**
     * A session did not come up. One holding a surface or a deferred output steps
     * down to the reader alone, the CPU path that is known to fit. The reader alone
     * failing means the camera cannot be used at all.
     */
    private fun sessionFailed(
        session: Session,
        token: Long,
        surface: Surface?,
        wasDeferred: Boolean,
        why: String,
    ) {
        session.configuring = false
        if (session.closed) return
        Log.w(TAG, "session failed (" + describeOutputs(surface, null, session.previewSize) +
            ", deferred=" + wasDeferred + "): " + why)
        if (surface != null) {
            Log.w(TAG, "preview refused: this stream displays through CPU frames")
            session.previewSurface = null
            nativePreviewRefused(token)
            configureSession(session, token)
        } else if (wasDeferred) {
            configureSession(session, token)
        } else {
            cleanup(token)
            nativeError(token, ERR_EXCEPTION, "capture session configuration failed: $why")
        }
    }

    private fun describeOutputs(
        surface: Surface?,
        deferred: OutputConfiguration?,
        previewSize: Size?,
    ): String = when {
        surface != null -> "preview $previewSize + reader"
        deferred != null -> "deferred preview $previewSize + reader"
        else -> "reader only"
    }

    /**
     * Bring the running session in line with the attached surface. Filling the
     * deferred output needs no rebuild; a surface the session has no output for does.
     */
    private fun linkPreview(session: Session, token: Long) {
        if (session.configuring) return // onConfigured comes back here.
        val surface = session.previewSurface
        if (surface != null && surface !== session.sessionSurface) {
            val deferred = session.deferredOutput
            val captureSession = session.captureSession
            if (deferred != null && captureSession != null &&
                fillDeferredOutput(session, captureSession, deferred, surface)
            ) {
                Log.i(TAG, "preview attached: deferred output filled, no rebuild")
            } else {
                Log.i(TAG, "preview attached: rebuilding the session for it")
                configureSession(session, token)
                return
            }
        }
        applyTargets(session)
    }

    private fun fillDeferredOutput(
        session: Session,
        captureSession: CameraCaptureSession,
        deferred: OutputConfiguration,
        surface: Surface,
    ): Boolean {
        session.deferredOutput = null
        return try {
            deferred.addSurface(surface)
            captureSession.finalizeOutputConfigurations(listOf(deferred))
            session.sessionSurface = surface
            true
        } catch (t: Throwable) {
            Log.w(TAG, "deferred output refused: $t")
            false
        }
    }

    /** Point the repeating request at what is wanted now. Runs on the capture thread. */
    private fun applyTargets(session: Session) {
        if (session.configuring || session.closed) return
        val device = session.device ?: return
        val captureSession = session.captureSession ?: return
        val readerSurface = session.reader?.surface ?: return
        val live = session.previewSurface?.takeIf { it === session.sessionSurface }
        val targets = nativeRepeatingTargets(live != null, session.cpuWanted)
        val builder = device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW)
        if (targets and TARGET_PREVIEW != 0 && live != null) builder.addTarget(live)
        if (targets and TARGET_READER != 0) builder.addTarget(readerSurface)
        if (session.fps > 0) {
            builder.set(
                CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE,
                Range(session.fps, session.fps)
            )
        }
        captureSession.setRepeatingRequest(builder.build(), null, session.handler)
        Log.d(TAG, "repeating: preview=" + (targets and TARGET_PREVIEW != 0) +
            " reader=" + (targets and TARGET_READER != 0) + " cpu=" + session.cpuWanted)
    }

    /** Run [body] on [token]'s capture thread, if that stream is still open. */
    private fun onCaptureThread(token: Long, body: (Session) -> Unit) {
        val session = sessions[token] ?: return
        session.handler?.post {
            if (session.closed) return@post
            try {
                body(session)
            } catch (t: Throwable) {
                // A session closing under a request: nothing to act on, the next
                // session or the close sets things straight.
                Log.w(TAG, "request not applied: $t")
            }
        }
    }

    /**
     * A display's [surface] is ready. It must be sized to the preview size (from
     * [nativeOpened]) before this call. Replaces any previous surface.
     */
    @JvmStatic
    fun attachPreview(token: Long, surface: Surface) {
        onCaptureThread(token) { session ->
            session.previewSurface = surface
            linkPreview(session, token)
        }
    }

    /**
     * The display's surface is going away. Requests stop naming it; it stays an
     * output of this session, unused, until the next one is made. A display must keep
     * it alive for the frames already in flight to it.
     */
    @JvmStatic
    fun detachPreview(token: Long) {
        onCaptureThread(token) { session ->
            session.previewSurface = null
            applyTargets(session)
        }
    }

    /** Whether a consumer wants CPU frames. Called from any thread, on each change. */
    @JvmStatic
    fun setCpuDemand(token: Long, wanted: Boolean) {
        val session = sessions[token] ?: return
        session.cpuWanted = wanted
        onCaptureThread(token) { applyTargets(it) }
    }

    /** Resolve [facing] to a camera id, or null if none matches. */
    private fun pickCameraId(manager: CameraManager, facing: Int): String? {
        val want = when (facing) {
            FACING_FRONT -> CameraCharacteristics.LENS_FACING_FRONT
            FACING_BACK -> CameraCharacteristics.LENS_FACING_BACK
            else -> CameraCharacteristics.LENS_FACING_BACK // Default prefers back.
        }
        var firstId: String? = null
        for (id in manager.cameraIdList) {
            if (firstId == null) firstId = id
            val lens = manager.getCameraCharacteristics(id)
                .get(CameraCharacteristics.LENS_FACING)
            if (lens == want) return id
        }
        // Default falls back to any camera; an explicit front/back that isn't
        // present is reported as "no camera".
        return if (facing == FACING_DEFAULT) firstId else null
    }

    /**
     * Pick the output size. An explicit [width]x[height] must match exactly
     * (else null → UnsupportedConfig); otherwise choose the largest size not
     * exceeding 1080p, falling back to the largest available.
     */
    private fun chooseSize(sizes: Array<Size>, width: Int, height: Int): Size? {
        if (width > 0 && height > 0) {
            return sizes.firstOrNull { it.width == width && it.height == height }
        }
        val capped = sizes.filter { it.width <= 1920 && it.height <= 1080 }
        val pool = if (capped.isNotEmpty()) capped else sizes.asList()
        return pool.maxByOrNull { it.width.toLong() * it.height.toLong() }
    }

    /**
     * The size a display's surface takes for frames of [readerSize], or null if none
     * fits. The rules are `android_preview::choose_preview_size`.
     */
    private fun choosePreviewSize(
        context: Context,
        configMap: android.hardware.camera2.params.StreamConfigurationMap?,
        readerSize: Size,
    ): Size? {
        val sizes = configMap?.getOutputSizes(SurfaceTexture::class.java) ?: return null
        val flat = IntArray(sizes.size * 2)
        sizes.forEachIndexed { i, s ->
            flat[i * 2] = s.width
            flat[i * 2 + 1] = s.height
        }
        val display = Point()
        val dm = context.getSystemService(Context.DISPLAY_SERVICE) as? DisplayManager
        dm?.getDisplay(Display.DEFAULT_DISPLAY)?.getRealSize(display)
        val index =
            nativeChoosePreviewSize(readerSize.width, readerSize.height, flat, display.x, display.y)
        return if (index >= 0) sizes[index] else null
    }

    /**
     * Keep [Session.rotation] in step with the display. Called on the main
     * thread (from [start]); the listener runs there too.
     *
     * A `DisplayListener` rather than the runtime's
     * `Activity.onConfigurationChanged` path: a 180° turn (landscape →
     * reverse landscape, portrait → upside down) changes no configuration
     * axis, so `onConfigurationChanged` never fires for it, but the display's
     * rotation does change and `onDisplayChanged` reports it. It is also
     * self-contained — the SDK needs no hook in the generated Activity.
     * An `OrientationEventListener` would follow the physical sensor even when
     * the app's UI is rotation-locked; the DISPLAY rotation is what the user
     * sees as "up", matching web (the browser) and iOS (interface
     * orientation).
     */
    private fun followDisplayRotation(context: Context, session: Session) {
        val dm = context.getSystemService(Context.DISPLAY_SERVICE) as? DisplayManager
        fun update() {
            val display = dm?.getDisplay(Display.DEFAULT_DISPLAY)
            val displayRotation = display?.rotation ?: 0 // Surface.ROTATION_0
            session.rotation =
                nativeFrameRotation(session.sensorOrientation, displayRotation, session.front)
        }
        update()
        if (dm == null) return
        val listener = object : DisplayManager.DisplayListener {
            override fun onDisplayAdded(displayId: Int) {}
            override fun onDisplayRemoved(displayId: Int) {}
            override fun onDisplayChanged(displayId: Int) {
                if (displayId == Display.DEFAULT_DISPLAY) update()
            }
        }
        // `null` handler = the calling (main) looper.
        dm.registerDisplayListener(listener, null)
        session.displayManager = dm
        session.displayListener = listener
    }

    /** Tear down everything associated with [token]. Idempotent. */
    private fun cleanup(token: Long) {
        val session = sessions.remove(token) ?: return
        session.closed = true
        session.cpuWanted = false
        session.displayListener?.let { l -> session.displayManager?.unregisterDisplayListener(l) }
        session.displayListener = null
        try {
            session.captureSession?.stopRepeating()
        } catch (_: Throwable) {
        }
        try {
            session.captureSession?.close()
        } catch (_: Throwable) {
        }
        try {
            session.device?.close()
        } catch (_: Throwable) {
        }
        // The ImageReader closes on the conversion thread, after everything already
        // queued there. A frame may be mid-`nativeFrameYuv` reading the reader's
        // native Image buffers; `reader.close()` frees them, and reading freed native
        // memory is a use-after-free SIGSEGV (the "camera crashes the app a while
        // after starting" bug, which fires only when the session is torn down with a
        // frame in flight). Ordering through the threads themselves closes that hole
        // outright, where joining from here could only wait so long. Once the capture
        // thread has dropped the listener, nothing more gets queued; that runnable
        // queues the close behind any conversion already queued.
        val reader = session.reader
        val convertThread = session.convertThread
        val convertHandler = session.convertHandler
        val closeReader = {
            session.pendingImage.getAndSet(null)?.close()
            try {
                reader?.close()
            } catch (_: Throwable) {
            }
            convertThread?.quitSafely()
            Unit
        }
        val handler = session.handler
        val thread = session.thread
        if (handler != null) {
            handler.post {
                reader?.setOnImageAvailableListener(null, null)
                convertHandler?.post(closeReader)
                thread?.quitSafely()
            }
        } else {
            closeReader()
        }
    }

    /**
     * The session is live. A non-zero [previewWidth] x [previewHeight] is what a
     * display's surface can take; [sensorOrientation] and [front] say how to place
     * it. Zero means no native preview for this stream.
     */
    @JvmStatic
    private external fun nativeOpened(
        token: Long,
        previewWidth: Int,
        previewHeight: Int,
        sensorOrientation: Int,
        front: Boolean,
    )

    @JvmStatic
    private external fun nativeError(token: Long, code: Int, message: String?)

    /** The device will not run a preview beside the reader. Sent once per stream. */
    @JvmStatic
    private external fun nativePreviewRefused(token: Long)

    /** Which outputs to target, as `TARGET_*` bits. `android_preview::repeating_targets`. */
    @JvmStatic
    private external fun nativeRepeatingTargets(previewLive: Boolean, cpuWanted: Boolean): Int

    /**
     * Index into [sizes] (flat `[w0, h0, w1, h1, …]`) of the preview's size, or -1.
     * `android_preview::choose_preview_size`.
     */
    @JvmStatic
    private external fun nativeChoosePreviewSize(
        readerWidth: Int,
        readerHeight: Int,
        sizes: IntArray,
        displayWidth: Int,
        displayHeight: Int,
    ): Int

    /**
     * Clockwise frame rotation for [sensorOrientation] (degrees), the
     * display's `Surface.ROTATION_*` [displayRotation] and [front] facing.
     * Implemented in Rust (`android_rotation::frame_rotation`).
     */
    @JvmStatic
    private external fun nativeFrameRotation(
        sensorOrientation: Int,
        displayRotation: Int,
        front: Boolean,
    ): Int

    /**
     * One image: its plane buffers (direct, valid until the `Image` closes) and
     * strides, its size, and the clockwise degrees to rotate it by. Converted
     * to `RGBA8` and delivered in Rust (`android_yuv`, the host-tested
     * conversion). For 90°/270° the delivered frame is `height × width`. Never
     * mirrors: the front camera comes out unmirrored, matching web and iOS.
     */
    @JvmStatic
    private external fun nativeFrameYuv(
        token: Long,
        y: java.nio.ByteBuffer,
        yRowStride: Int,
        yPixelStride: Int,
        u: java.nio.ByteBuffer,
        uRowStride: Int,
        uPixelStride: Int,
        v: java.nio.ByteBuffer,
        vRowStride: Int,
        vPixelStride: Int,
        width: Int,
        height: Int,
        rotation: Int,
    )
}
