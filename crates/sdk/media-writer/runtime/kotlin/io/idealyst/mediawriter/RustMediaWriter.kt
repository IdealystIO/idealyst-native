package io.idealyst.mediawriter

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.MediaMuxer
import android.util.Log
import java.nio.ByteBuffer
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

/**
 * Drives Android **MediaCodec** (H.264 video + AAC audio) and **MediaMuxer**
 * for the `media-writer` SDK. Rust forwards each captured RGBA frame / 16-bit
 * PCM chunk via [writeVideo] / [writeAudio]; this shim encodes and muxes them
 * into the `.mp4` at the path given to [start], lip-syncing the two tracks by
 * the microsecond capture timestamps Rust passes through.
 *
 * The encode/mux machinery (input/output buffer dequeue loops, `Image`-plane
 * YUV filling, two-codec muxer-start coordination) is far cleaner in Kotlin
 * than raw JNI, which is why it lives here. Shipped from the `media-writer`
 * crate via `[package.metadata.idealyst.android].runtime_kotlin`.
 *
 * VERIFICATION: compile-checked against the SDK's JNI signatures; the
 * MediaCodec/MediaMuxer path itself resolves only at runtime on a device.
 */
object RustMediaWriter {
    private const val TAG = "RustMediaWriter"
    private val recorders = ConcurrentHashMap<Long, Recorder>()
    private val nextToken = AtomicLong(1)

    @JvmStatic
    fun start(
        path: String,
        hasVideo: Boolean,
        hasAudio: Boolean,
        fps: Int,
        videoBitrate: Int,
        audioBitrate: Int,
    ): Long {
        return try {
            val token = nextToken.getAndIncrement()
            recorders[token] = Recorder(path, hasVideo, hasAudio, fps, videoBitrate, audioBitrate)
            token
        } catch (e: Throwable) {
            Log.e(TAG, "start failed", e)
            0L
        }
    }

    @JvmStatic
    fun writeVideo(token: Long, rgba: ByteArray, width: Int, height: Int, ptsUs: Long) {
        recorders[token]?.onVideo(rgba, width, height, ptsUs)
    }

    @JvmStatic
    fun writeAudio(token: Long, pcm16: ByteArray, sampleRate: Int, channels: Int, ptsUs: Long) {
        recorders[token]?.onAudio(pcm16, sampleRate, channels, ptsUs)
    }

    @JvmStatic
    fun stop(token: Long): Boolean {
        val rec = recorders.remove(token) ?: return false
        return rec.finish()
    }

    @JvmStatic
    fun abort(token: Long) {
        recorders.remove(token)?.abort()
    }
}

private class Recorder(
    private val path: String,
    private val hasVideo: Boolean,
    private val hasAudio: Boolean,
    private val fps: Int,
    private val videoBitrate: Int,
    private val audioBitrate: Int,
) {
    private val lock = Any()
    private val muxer = MediaMuxer(path, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4)
    private var muxerStarted = false

    private var videoCodec: MediaCodec? = null
    private var videoTrack = -1
    private var videoConfigured = false

    private var audioCodec: MediaCodec? = null
    private var audioTrack = -1
    private var audioConfigured = false

    private val bufferInfo = MediaCodec.BufferInfo()

    // --- Video ---------------------------------------------------------------

    fun onVideo(rgba: ByteArray, width: Int, height: Int, ptsUs: Long) {
        try {
            val codec = videoCodec ?: configureVideo(width, height)
            val index = codec.dequeueInputBuffer(0)
            if (index >= 0) {
                val image = codec.getInputImage(index)
                if (image != null) {
                    fillImageFromRgba(image, rgba, width, height)
                    codec.queueInputBuffer(index, 0, width * height * 3 / 2, ptsUs, 0)
                } else {
                    codec.queueInputBuffer(index, 0, 0, ptsUs, 0)
                }
            }
            drain(codec, video = true, endOfStream = false)
        } catch (e: Throwable) {
            Log.e("RustMediaWriter", "onVideo", e)
        }
    }

    private fun configureVideo(width: Int, height: Int): MediaCodec {
        val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height)
        format.setInteger(
            MediaFormat.KEY_COLOR_FORMAT,
            MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible,
        )
        format.setInteger(
            MediaFormat.KEY_BIT_RATE,
            if (videoBitrate > 0) videoBitrate else (width * height * 4),
        )
        format.setInteger(MediaFormat.KEY_FRAME_RATE, if (fps > 0) fps else 30)
        format.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
        val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        codec.start()
        videoCodec = codec
        videoConfigured = true
        return codec
    }

    /** BT.601 RGBA → YUV420, written into a (NV12 or I420) [MediaCodec.Image]. */
    private fun fillImageFromRgba(
        image: android.media.Image,
        rgba: ByteArray,
        width: Int,
        height: Int,
    ) {
        val yP = image.planes[0]
        val uP = image.planes[1]
        val vP = image.planes[2]
        val yBuf = yP.buffer
        val uBuf = uP.buffer
        val vBuf = vP.buffer
        val yRow = yP.rowStride
        val uRow = uP.rowStride
        val vRow = vP.rowStride
        val uPix = uP.pixelStride
        val vPix = vP.pixelStride

        var i = 0
        for (y in 0 until height) {
            for (x in 0 until width) {
                val r = rgba[i].toInt() and 0xff
                val g = rgba[i + 1].toInt() and 0xff
                val b = rgba[i + 2].toInt() and 0xff
                i += 4
                val yy = (0.299 * r + 0.587 * g + 0.114 * b).toInt().coerceIn(0, 255)
                yBuf.put(y * yRow + x, yy.toByte())
                if (x and 1 == 0 && y and 1 == 0) {
                    val u = (-0.169 * r - 0.331 * g + 0.5 * b + 128.0).toInt().coerceIn(0, 255)
                    val v = (0.5 * r - 0.419 * g - 0.081 * b + 128.0).toInt().coerceIn(0, 255)
                    val cx = x / 2
                    val cy = y / 2
                    uBuf.put(cy * uRow + cx * uPix, u.toByte())
                    vBuf.put(cy * vRow + cx * vPix, v.toByte())
                }
            }
        }
    }

    // --- Audio ---------------------------------------------------------------

    fun onAudio(pcm16: ByteArray, sampleRate: Int, channels: Int, ptsUs: Long) {
        try {
            val codec = audioCodec ?: configureAudio(sampleRate, channels)
            val index = codec.dequeueInputBuffer(0)
            if (index >= 0) {
                val buf = codec.getInputBuffer(index)
                if (buf != null) {
                    buf.clear()
                    buf.put(pcm16)
                    codec.queueInputBuffer(index, 0, pcm16.size, ptsUs, 0)
                }
            }
            drain(codec, video = false, endOfStream = false)
        } catch (e: Throwable) {
            Log.e("RustMediaWriter", "onAudio", e)
        }
    }

    private fun configureAudio(sampleRate: Int, channels: Int): MediaCodec {
        val format = MediaFormat.createAudioFormat(
            MediaFormat.MIMETYPE_AUDIO_AAC,
            sampleRate,
            channels,
        )
        format.setInteger(
            MediaFormat.KEY_AAC_PROFILE,
            MediaCodecInfo.CodecProfileLevel.AACObjectLC,
        )
        format.setInteger(
            MediaFormat.KEY_BIT_RATE,
            if (audioBitrate > 0) audioBitrate else 128_000,
        )
        val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_AAC)
        codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        codec.start()
        audioCodec = codec
        audioConfigured = true
        return codec
    }

    // --- Drain + mux ---------------------------------------------------------

    private fun drain(codec: MediaCodec, video: Boolean, endOfStream: Boolean) {
        while (true) {
            val index = codec.dequeueOutputBuffer(bufferInfo, if (endOfStream) 10_000 else 0)
            when {
                index == MediaCodec.INFO_TRY_AGAIN_LATER -> {
                    if (!endOfStream) return
                    // Keep polling until EOS flushes.
                }
                index == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                    synchronized(lock) {
                        val track = muxer.addTrack(codec.outputFormat)
                        if (video) videoTrack = track else audioTrack = track
                        maybeStartMuxer()
                    }
                }
                index >= 0 -> {
                    val buf = codec.getOutputBuffer(index)
                    val isConfig =
                        bufferInfo.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0
                    if (buf != null && !isConfig && bufferInfo.size > 0) {
                        synchronized(lock) {
                            if (muxerStarted) {
                                buf.position(bufferInfo.offset)
                                buf.limit(bufferInfo.offset + bufferInfo.size)
                                val track = if (video) videoTrack else audioTrack
                                if (track >= 0) {
                                    muxer.writeSampleData(track, buf, bufferInfo)
                                }
                            }
                        }
                    }
                    codec.releaseOutputBuffer(index, false)
                    if (bufferInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) return
                }
            }
        }
    }

    /** Start the muxer once every expected track has been added. */
    private fun maybeStartMuxer() {
        if (muxerStarted) return
        val haveVideo = !hasVideo || videoTrack >= 0
        val haveAudio = !hasAudio || audioTrack >= 0
        if (haveVideo && haveAudio) {
            muxer.start()
            muxerStarted = true
        }
    }

    // --- Finalize ------------------------------------------------------------

    fun finish(): Boolean {
        return try {
            videoCodec?.let { signalEosAndDrain(it, video = true) }
            audioCodec?.let { signalEosAndDrain(it, video = false) }
            synchronized(lock) {
                if (muxerStarted) {
                    muxer.stop()
                }
            }
            releaseAll()
            true
        } catch (e: Throwable) {
            Log.e("RustMediaWriter", "finish", e)
            releaseAll()
            false
        }
    }

    private fun signalEosAndDrain(codec: MediaCodec, video: Boolean) {
        val index = codec.dequeueInputBuffer(10_000)
        if (index >= 0) {
            codec.queueInputBuffer(index, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
        }
        drain(codec, video, endOfStream = true)
    }

    fun abort() {
        try {
            releaseAll()
        } catch (_: Throwable) {
        }
        java.io.File(path).delete()
    }

    private fun releaseAll() {
        try {
            videoCodec?.stop(); videoCodec?.release()
        } catch (_: Throwable) {
        }
        try {
            audioCodec?.stop(); audioCodec?.release()
        } catch (_: Throwable) {
        }
        try {
            muxer.release()
        } catch (_: Throwable) {
        }
        videoCodec = null
        audioCodec = null
    }
}
