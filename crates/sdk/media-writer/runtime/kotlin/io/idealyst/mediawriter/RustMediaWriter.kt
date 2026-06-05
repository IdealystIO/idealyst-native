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

    // `rgba` / `pcm16` are direct ByteBuffers the Rust side hands over
    // zero-copy (they view Rust-owned memory for this synchronous call); the
    // encoder reads out of them and never retains them.
    @JvmStatic
    fun writeVideo(token: Long, rgba: ByteBuffer, width: Int, height: Int, ptsUs: Long) {
        recorders[token]?.onVideo(rgba, width, height, ptsUs)
    }

    @JvmStatic
    fun writeAudio(token: Long, pcm16: ByteBuffer, sampleRate: Int, channels: Int, ptsUs: Long) {
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
    // Source frame size vs the (possibly down-scaled, alignment-corrected)
    // ENCODED size the AVC encoder actually accepts. Fixed at first frame.
    private var srcW = 0
    private var srcH = 0
    private var encW = 0
    private var encH = 0

    private var audioCodec: MediaCodec? = null
    private var audioTrack = -1
    private var audioConfigured = false

    private val bufferInfo = MediaCodec.BufferInfo()

    // Last presentation timestamp written to each track. `MediaMuxer` rejects
    // non-monotonic timestamps with "writeSampleData returned an error", and
    // the audio capture clock can briefly deliver out-of-order PTS at startup
    // (the AAC encoder logs its own "overlapping timestamp" correction). We
    // clamp each sample to be strictly after the previous one on its track so
    // a little PTS jitter doesn't abort the recording.
    private var lastVideoPtsUs = -1L
    private var lastAudioPtsUs = -1L

    // --- Video ---------------------------------------------------------------

    fun onVideo(rgba: ByteBuffer, width: Int, height: Int, ptsUs: Long) {
        try {
            // First frame fixes the encoded size (`encW`/`encH`); later frames
            // reuse it. Frames are scaled from the source size into that target.
            val codec = videoCodec ?: configureVideo(width, height)
            if (encW <= 0 || encH <= 0) return
            val index = codec.dequeueInputBuffer(0)
            if (index >= 0) {
                val image = codec.getInputImage(index)
                if (image != null) {
                    fillImageFromRgba(image, rgba, encW, encH, srcW, srcH)
                    codec.queueInputBuffer(index, 0, encW * encH * 3 / 2, ptsUs, 0)
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
        val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        // The AVC encoder rejects (IllegalArgumentException / EINVAL) any size
        // that's odd OR larger than it supports — a full-screen self-capture
        // (e.g. 1440x2891) blows past the emulator encoder's ~1920px ceiling.
        // Scale DOWN to fit the encoder's reported max, preserving aspect, then
        // snap to its required width/height alignment (>= even). `srcW/srcH` keep
        // the source size so `fillImageFromRgba` can nearest-neighbor sample.
        val vc = codec.codecInfo
            .getCapabilitiesForType(MediaFormat.MIMETYPE_VIDEO_AVC)
            .videoCapabilities
        val maxW = vc.supportedWidths.upper
        val maxH = vc.supportedHeights.upper
        val scale = minOf(1.0, maxW.toDouble() / width, maxH.toDouble() / height)
        val wAlign = maxOf(2, vc.widthAlignment)
        val hAlign = maxOf(2, vc.heightAlignment)
        val w = (((width * scale).toInt()) / wAlign * wAlign).coerceAtLeast(wAlign)
        val h = (((height * scale).toInt()) / hAlign * hAlign).coerceAtLeast(hAlign)
        srcW = width
        srcH = height
        encW = w
        encH = h

        val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, w, h)
        format.setInteger(
            MediaFormat.KEY_COLOR_FORMAT,
            MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible,
        )
        format.setInteger(
            MediaFormat.KEY_BIT_RATE,
            if (videoBitrate > 0) videoBitrate else (w * h * 4),
        )
        format.setInteger(MediaFormat.KEY_FRAME_RATE, if (fps > 0) fps else 30)
        format.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
        codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        codec.start()
        videoCodec = codec
        videoConfigured = true
        return codec
    }

    /** BT.601 RGBA → YUV420, written into a (NV12 or I420) [MediaCodec.Image]. */
    private fun fillImageFromRgba(
        image: android.media.Image,
        rgba: ByteBuffer,
        width: Int,
        height: Int,
        srcWidth: Int,
        srcHeight: Int,
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

        // `width`/`height` are the ENCODED size; sample the `srcWidth`×`srcHeight`
        // RGBA with nearest-neighbor scaling (a no-op when sizes match — the
        // common case — so unscaled recordings stay pixel-exact).
        for (y in 0 until height) {
            val sy = if (height == srcHeight) y else y * srcHeight / height
            for (x in 0 until width) {
                val sx = if (width == srcWidth) x else x * srcWidth / width
                val i = (sy * srcWidth + sx) * 4
                val r = rgba.get(i).toInt() and 0xff
                val g = rgba.get(i + 1).toInt() and 0xff
                val b = rgba.get(i + 2).toInt() and 0xff
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

    fun onAudio(pcm16: ByteBuffer, sampleRate: Int, channels: Int, ptsUs: Long) {
        try {
            val codec = audioCodec ?: configureAudio(sampleRate, channels)
            pcm16.rewind()
            val total = pcm16.remaining()
            // 16-bit interleaved samples: one sample-frame is `channels` shorts.
            // We split on frame boundaries so a sample is never cut between AAC
            // input buffers.
            val bytesPerFrame = (channels * 2).coerceAtLeast(2)
            var offset = 0
            // A mic chunk can be larger than a single codec input buffer, so
            // feed it across as many buffers as it takes (the old single-`put`
            // path threw BufferOverflowException on the first oversized chunk).
            while (offset < total) {
                val index = codec.dequeueInputBuffer(10_000)
                if (index < 0) {
                    // No free input buffer — drain encoded output to release
                    // one, then retry rather than dropping the audio.
                    drain(codec, video = false, endOfStream = false)
                    continue
                }
                val buf = codec.getInputBuffer(index)
                if (buf == null) {
                    codec.queueInputBuffer(index, 0, 0, ptsUs, 0)
                    continue
                }
                buf.clear()
                var chunk = minOf(buf.capacity(), total - offset)
                chunk -= chunk % bytesPerFrame
                if (chunk <= 0) break // sub-frame tail (shouldn't happen on aligned input)
                // PTS advances by the duration of the samples already fed this call.
                val framesConsumed = (offset / bytesPerFrame).toLong()
                val chunkPts = ptsUs + framesConsumed * 1_000_000L / sampleRate
                pcm16.clear()
                pcm16.position(offset)
                pcm16.limit(offset + chunk)
                buf.put(pcm16)
                codec.queueInputBuffer(index, 0, chunk, chunkPts, 0)
                offset += chunk
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
                                    // Enforce strictly-increasing PTS per track —
                                    // MediaMuxer errors otherwise.
                                    val last = if (video) lastVideoPtsUs else lastAudioPtsUs
                                    if (bufferInfo.presentationTimeUs <= last) {
                                        bufferInfo.presentationTimeUs = last + 1
                                    }
                                    if (video) lastVideoPtsUs = bufferInfo.presentationTimeUs
                                    else lastAudioPtsUs = bufferInfo.presentationTimeUs
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
