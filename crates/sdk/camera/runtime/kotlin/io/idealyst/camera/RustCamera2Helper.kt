package io.idealyst.camera

import android.content.Context
import android.graphics.ImageFormat
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.media.Image
import android.media.ImageReader
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.util.Range
import android.util.Size
import java.util.concurrent.ConcurrentHashMap

/**
 * Bridges Android **Camera2** + **ImageReader** to Rust for the `camera`
 * SDK. Rust calls [open] with a `token` identifying the awaiting native
 * stream; the shim opens the camera, streams `YUV_420_888` frames, converts
 * each to tightly-packed top-down `RGBA8`, and trampolines them back through
 * [nativeFrame]. Lifecycle (success / failure) goes through [nativeOpened] /
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
        // SENSOR_ORIENTATION (0/90/180/270): the camera sensor is mounted
        // landscape, so frames arrive rotated by this amount relative to a
        // portrait-held device. We rotate the RGBA output by it so upright
        // frames reach the FrameWriter — the Android analog of the iOS
        // AVCaptureConnection.videoOrientation fix.
        var sensorOrientation: Int = 0
    }

    private val sessions = ConcurrentHashMap<Long, Session>()

    @JvmStatic
    fun open(
        context: Context,
        facing: Int,
        width: Int,
        height: Int,
        fps: Int,
        token: Long,
    ) {
        Handler(Looper.getMainLooper()).post {
            try {
                start(context, facing, width, height, fps, token)
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
        facing: Int,
        width: Int,
        height: Int,
        fps: Int,
        token: Long,
    ) {
        val manager = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
        val cameraId = pickCameraId(manager, facing)
        if (cameraId == null) {
            nativeError(token, ERR_NO_CAMERA, "no camera matching facing=$facing")
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
        sessions[token] = session

        val thread = HandlerThread("idealyst-camera-$token").also { it.start() }
        val handler = Handler(thread.looper)
        session.thread = thread
        session.handler = handler

        val reader = ImageReader.newInstance(size.width, size.height, ImageFormat.YUV_420_888, 2)
        session.reader = reader
        reader.setOnImageAvailableListener({ r ->
            val image = r.acquireLatestImage() ?: return@setOnImageAvailableListener
            try {
                val rot = session.sensorOrientation
                val rgba = yuv420ToRgba(image, rot)
                // 90°/270° rotation swaps the frame's width and height.
                val swap = rot == 90 || rot == 270
                val outW = if (swap) image.height else image.width
                val outH = if (swap) image.width else image.height
                nativeFrame(token, rgba, outW, outH)
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

    /** Tear down everything associated with [token]. Idempotent. */
    private fun cleanup(token: Long) {
        val session = sessions.remove(token) ?: return
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
        // ImageReader. A frame may be mid-`yuv420ToRgba` on that thread reading
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

    /**
     * Convert a `YUV_420_888` [image] to tightly-packed top-down `RGBA8`
     * (BT.601 full-range), rotating the output by [rotation] degrees clockwise
     * (the camera's `SENSOR_ORIENTATION`) so frames are upright on a
     * portrait-held device. The rotation is baked into the destination index —
     * no extra copy pass. O(width*height); a future GPU/RenderScript path can
     * replace this without touching the Rust side.
     *
     * For 90°/270° the output is `height × width` (dimensions swapped); the
     * caller emits the swapped size to [nativeFrame]. Like the iOS fix this
     * assumes a portrait device and does not mirror the front camera.
     */
    private fun yuv420ToRgba(image: Image, rotation: Int): ByteArray {
        val width = image.width
        val height = image.height
        val out = ByteArray(width * height * 4)
        // Output row stride (in pixels): swapped for 90°/270°.
        val outW = if (rotation == 90 || rotation == 270) height else width

        val yPlane = image.planes[0]
        val uPlane = image.planes[1]
        val vPlane = image.planes[2]

        val yBuf = yPlane.buffer
        val uBuf = uPlane.buffer
        val vBuf = vPlane.buffer

        val yRowStride = yPlane.rowStride
        val yPixelStride = yPlane.pixelStride
        val uRowStride = uPlane.rowStride
        val uPixelStride = uPlane.pixelStride
        val vRowStride = vPlane.rowStride
        val vPixelStride = vPlane.pixelStride

        for (row in 0 until height) {
            val yRow = row * yRowStride
            val uvRow = (row shr 1)
            for (col in 0 until width) {
                val y = (yBuf.get(yRow + col * yPixelStride).toInt() and 0xFF)
                val uIndex = uvRow * uRowStride + (col shr 1) * uPixelStride
                val vIndex = uvRow * vRowStride + (col shr 1) * vPixelStride
                val u = (uBuf.get(uIndex).toInt() and 0xFF) - 128
                val v = (vBuf.get(vIndex).toInt() and 0xFF) - 128

                var r = y + ((1436 * v) shr 10)
                var g = y - ((352 * u + 731 * v) shr 10)
                var b = y + ((1814 * u) shr 10)

                if (r < 0) r = 0 else if (r > 255) r = 255
                if (g < 0) g = 0 else if (g > 255) g = 255
                if (b < 0) b = 0 else if (b > 255) b = 255

                // Destination pixel in the rotated frame. Clockwise by
                // `rotation`; 90°/270° map into the height×width output.
                val dRow: Int
                val dCol: Int
                when (rotation) {
                    90 -> { dRow = col; dCol = height - 1 - row }
                    180 -> { dRow = height - 1 - row; dCol = width - 1 - col }
                    270 -> { dRow = width - 1 - col; dCol = row }
                    else -> { dRow = row; dCol = col }
                }
                val dst = (dRow * outW + dCol) * 4

                out[dst] = r.toByte()
                out[dst + 1] = g.toByte()
                out[dst + 2] = b.toByte()
                out[dst + 3] = 255.toByte()
            }
        }
        return out
    }

    @JvmStatic
    private external fun nativeOpened(token: Long)

    @JvmStatic
    private external fun nativeError(token: Long, code: Int, message: String?)

    @JvmStatic
    private external fun nativeFrame(token: Long, data: ByteArray, width: Int, height: Int)
}
