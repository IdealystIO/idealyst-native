package io.idealyst.videocodec

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.view.Surface
import java.util.concurrent.ConcurrentHashMap

/**
 * Realtime H.264 for the `video-codec` SDK: a MediaCodec encoder fed through
 * its input surface, which Rust hands to a capture (a stream's
 * `PreviewSource`) to render into. Each encoded frame goes back through
 * [nativeFrame] on the codec's own thread; a keyframe carries the SPS and PPS
 * in front, so a decoder can start at any keyframe.
 *
 * Shipped from the `video-codec` SDK crate via
 * `[package.metadata.idealyst.android].runtime_kotlin`; the `native*` symbols
 * are the `#[no_mangle]` exports in `android.rs`.
 */
object RustVideoEncoder {
    private const val MIME = "video/avc"

    private class Session(val codec: MediaCodec, val surface: Surface, val thread: HandlerThread) {
        @Volatile var config: ByteArray? = null
        @Volatile var running = true
    }

    private val sessions = ConcurrentHashMap<Long, Session>()
    private val main by lazy { Handler(Looper.getMainLooper()) }

    /** Make and start an encoder for [token]; its input surface, for the capture to render into. */
    @JvmStatic
    fun start(token: Long, width: Int, height: Int, bitrate: Int, fps: Int): Surface {
        val thread = HandlerThread("idealyst-video-encoder-$token").also { it.start() }
        val codec = MediaCodec.createEncoderByType(MIME)
        try {
            codec.setCallback(callback(token), Handler(thread.looper))
            configure(codec, width, height, bitrate, fps)
            val surface = codec.createInputSurface()
            sessions[token] = Session(codec, surface, thread)
            codec.start()
            return surface
        } catch (t: Throwable) {
            sessions.remove(token)
            try { codec.release() } catch (_: Throwable) {}
            thread.quitSafely()
            throw t
        }
    }

    @JvmStatic
    fun requestKeyframe(token: Long) {
        val s = sessions[token] ?: return
        try {
            s.codec.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
        } catch (_: Throwable) {
        }
    }

    /**
     * Stop and release on the main thread: the capture's detach was posted
     * there first, so it has stopped rendering into the surface by then.
     */
    @JvmStatic
    fun stop(token: Long) {
        main.post { release(token) }
    }

    private fun release(token: Long) {
        val s = sessions.remove(token) ?: return
        s.running = false
        try { s.codec.stop() } catch (_: Throwable) {}
        try { s.codec.release() } catch (_: Throwable) {}
        try { s.surface.release() } catch (_: Throwable) {}
        s.thread.quitSafely()
    }

    private fun callback(token: Long) = object : MediaCodec.Callback() {
        override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {}

        override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
            val s = sessions[token] ?: return
            if (!s.running) return
            try {
                val buffer = codec.getOutputBuffer(index)
                if (buffer != null && info.size > 0) {
                    val bytes = ByteArray(info.size)
                    buffer.position(info.offset)
                    buffer.limit(info.offset + info.size)
                    buffer.get(bytes)
                    if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                        s.config = bytes
                    } else {
                        val key = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
                        val config = s.config
                        val frame = if (key && config != null && !startsWithSps(bytes)) config + bytes else bytes
                        nativeFrame(token, frame, info.presentationTimeUs, key)
                    }
                }
                codec.releaseOutputBuffer(index, false)
            } catch (_: IllegalStateException) {
                // Stopped under us; the session is going.
            }
        }

        override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
            val s = sessions[token] ?: return
            if (!s.running) return
            s.running = false
            nativeEnded(token, "encoder: ${e.diagnosticInfo}")
            main.post { release(token) }
        }

        override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {}
    }

    /**
     * Realtime settings: baseline (no B-frames, so nothing waits on a later
     * frame), keyframes when asked rather than on a timer, and the last frame
     * repeated while the picture is still, so a receiver that lost a frame is
     * not left waiting for it to change. An encoder that refuses the profile
     * gets the rest without it.
     */
    private fun configure(codec: MediaCodec, width: Int, height: Int, bitrate: Int, fps: Int) {
        fun format(baseline: Boolean) = MediaFormat.createVideoFormat(MIME, width, height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, bitrate)
            setInteger(MediaFormat.KEY_FRAME_RATE, fps)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 10)
            setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 250_000)
            setInteger(MediaFormat.KEY_PRIORITY, 0)
            if (Build.VERSION.SDK_INT >= 29) setInteger(MediaFormat.KEY_PREPEND_HEADER_TO_SYNC_FRAMES, 1)
            if (Build.VERSION.SDK_INT >= 30) setInteger(MediaFormat.KEY_LATENCY, 1)
            val caps = codec.codecInfo.getCapabilitiesForType(MIME).encoderCapabilities
            if (caps.isBitrateModeSupported(MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)) {
                setInteger(MediaFormat.KEY_BITRATE_MODE, MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
            }
            if (baseline) setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline)
        }
        try {
            codec.configure(format(true), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        } catch (_: Throwable) {
            codec.reset()
            codec.configure(format(false), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        }
    }

    /** Whether an Annex B frame opens with a sequence parameter set (NAL type 7). */
    private fun startsWithSps(b: ByteArray): Boolean {
        val at = when {
            b.size > 4 && b[0].toInt() == 0 && b[1].toInt() == 0 && b[2].toInt() == 0 && b[3].toInt() == 1 -> 4
            b.size > 3 && b[0].toInt() == 0 && b[1].toInt() == 0 && b[2].toInt() == 1 -> 3
            else -> return false
        }
        return b[at].toInt() and 0x1f == 7
    }

    @JvmStatic private external fun nativeFrame(token: Long, data: ByteArray, ptsUs: Long, key: Boolean)
    @JvmStatic private external fun nativeEnded(token: Long, message: String?)
}
