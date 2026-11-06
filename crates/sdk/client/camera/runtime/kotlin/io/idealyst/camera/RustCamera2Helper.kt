package io.idealyst.camera

import android.content.Context
import android.graphics.ImageFormat
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.hardware.display.DisplayManager
import android.media.Image
import android.media.ImageReader
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.util.Range
import android.util.Size
import android.view.Display
import java.util.concurrent.ConcurrentHashMap

/**
 * Bridges Android **Camera2** + **ImageReader** to Rust for the `camera`
 * SDK. Rust calls [open] with a `token` identifying the awaiting native
 * stream; the shim opens the camera, streams `YUV_420_888` frames, and hands
 * each image's planes to Rust through [nativeFrameYuv], which converts them to
 * tightly-packed top-down `RGBA8`. Lifecycle (success / failure) goes through [nativeOpened] /
 * [nativeError]. [close] tears the session down.
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

    private class Session {
        var device: CameraDevice? = null
        var captureSession: CameraCaptureSession? = null
        var reader: ImageReader? = null
        var thread: HandlerThread? = null
        var handler: Handler? = null
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
        session.sensorOrientation =
            characteristics.get(CameraCharacteristics.SENSOR_ORIENTATION) ?: 0
        session.front = characteristics.get(CameraCharacteristics.LENS_FACING) ==
            CameraCharacteristics.LENS_FACING_FRONT
        sessions[token] = session
        followDisplayRotation(context, session)

        val thread = HandlerThread("idealyst-camera-$token").also { it.start() }
        val handler = Handler(thread.looper)
        session.thread = thread
        session.handler = handler

        val reader = ImageReader.newInstance(size.width, size.height, ImageFormat.YUV_420_888, 2)
        session.reader = reader
        reader.setOnImageAvailableListener({ r ->
            val image = r.acquireLatestImage() ?: return@setOnImageAvailableListener
            try {
                // Conversion runs in Rust: a per-pixel `ByteBuffer` loop here
                // pins this thread at about one frame a second at 1080p.
                val (y, u, v) = image.planes
                // One read per frame: a rotation landing mid-frame applies
                // from the next frame, never half-way through this one.
                nativeFrameYuv(
                    token,
                    y.buffer,
                    u.buffer,
                    v.buffer,
                    intArrayOf(
                        y.rowStride, y.pixelStride,
                        u.rowStride, u.pixelStride,
                        v.rowStride, v.pixelStride,
                    ),
                    image.width,
                    image.height,
                    session.rotation,
                )
            } catch (_: Throwable) {
                // Drop the frame; never throw into the listener.
            } finally {
                image.close()
            }
        }, handler)

        // openCamera requires CAMERA permission (checked on the Rust side);
        // SecurityException here would be surfaced via the catch in open().
        manager.openCamera(cameraId, object : CameraDevice.StateCallback() {
            override fun onOpened(device: CameraDevice) {
                session.device = device
                try {
                    configureSession(session, device, reader, fps, token)
                } catch (t: Throwable) {
                    cleanup(token)
                    nativeError(token, ERR_EXCEPTION, t.message ?: t.toString())
                }
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

    private fun configureSession(
        session: Session,
        device: CameraDevice,
        reader: ImageReader,
        fps: Int,
        token: Long,
    ) {
        val surface = reader.surface
        device.createCaptureSession(
            listOf(surface),
            object : CameraCaptureSession.StateCallback() {
                override fun onConfigured(captureSession: CameraCaptureSession) {
                    session.captureSession = captureSession
                    try {
                        val builder =
                            device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW)
                        builder.addTarget(surface)
                        if (fps > 0) {
                            builder.set(
                                CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE,
                                Range(fps, fps)
                            )
                        }
                        captureSession.setRepeatingRequest(builder.build(), null, session.handler)
                        nativeOpened(token)
                    } catch (t: Throwable) {
                        cleanup(token)
                        nativeError(token, ERR_EXCEPTION, t.message ?: t.toString())
                    }
                }

                override fun onConfigureFailed(captureSession: CameraCaptureSession) {
                    cleanup(token)
                    nativeError(token, ERR_EXCEPTION, "capture session configuration failed")
                }
            },
            session.handler
        )
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
        // Stop the capture thread and WAIT for it to drain BEFORE closing the
        // ImageReader. A frame may be mid-`nativeFrameYuv` on that thread reading
        // the reader's native Image buffers; `reader.close()` frees them, and
        // reading freed native memory is a use-after-free SIGSEGV (the "camera
        // crashes the app a while after starting" bug — only fires when the
        // session is torn down with a frame in flight). `quitSafely` lets the
        // in-flight `onImageAvailable` finish (closing its Image in `finally`);
        // `join` blocks this (main) thread until the looper has exited.
        session.thread?.let { t ->
            t.quitSafely()
            try {
                t.join(500)
            } catch (_: InterruptedException) {
            }
        }
        try {
            session.reader?.close()
        } catch (_: Throwable) {
        }
    }

    @JvmStatic
    private external fun nativeOpened(token: Long)

    @JvmStatic
    private external fun nativeError(token: Long, code: Int, message: String?)

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
     * One image's planes, valid for this call; [strides] holds each plane's row
     * then pixel stride, in Y, U, V order. Rust converts them to upright `RGBA8`,
     * rotated [rotation] degrees clockwise ([Session.rotation]).
     */
    @JvmStatic
    private external fun nativeFrameYuv(
        token: Long,
        y: java.nio.ByteBuffer,
        u: java.nio.ByteBuffer,
        v: java.nio.ByteBuffer,
        strides: IntArray,
        width: Int,
        height: Int,
        rotation: Int,
    )
}
