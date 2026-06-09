package io.idealyst.videodecode

import android.content.Context
import android.graphics.ImageFormat
import android.graphics.PixelFormat
import android.media.Image
import android.media.ImageReader
import android.media.MediaCodec
import android.media.MediaExtractor
import android.media.MediaFormat
import android.net.Uri
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Bridges Android **MediaExtractor + MediaCodec** decode to Rust for the
 * `video-decode` SDK — the Android analog of the Apple `AVPlayer` +
 * `AVPlayerItemVideoOutput` + `MTAudioProcessingTap` backend.
 *
 * Rust calls [open] with a `token` identifying the awaiting native streams. The
 * shim demuxes the source URL, decodes the first video track to an
 * [ImageReader] surface (RGBA_8888), converts each frame to tightly-packed
 * top-down RGBA8, and trampolines it through [nativeFrameDirect]; it decodes the
 * first audio track (if any) to PCM16, converts to interleaved f32, and
 * trampolines through [nativeAudio]. The video PTS, throttled to real time,
 * drives the playback clock; [play]/[pause]/[seek]/[setMuted]/[setRate] control
 * it and [nativeState] pushes position/duration/flags back so the Rust transport
 * getters read a cached value. Lifecycle (success / failure) goes through
 * [nativeOpened] / [nativeError]; [close] tears everything down.
 *
 * The decode loops and codec dequeue are blocking + thread-driven, which is why
 * this lives in Kotlin rather than raw JNI. Shipped from the `video-decode` SDK
 * crate via `[package.metadata.idealyst.android].runtime_kotlin`; the `native*`
 * symbols are the `#[no_mangle]` exports in `android.rs`.
 *
 * DEVICE-VERIFICATION TODOs are flagged inline with `// TODO(device):`.
 */
object RustVideoDecoder {
    // Error-code sentinels Rust's `map_open_error` recognises.
    private const val ERR_BAD_SOURCE = -2
    private const val ERR_EXCEPTION = -1

    private const val DEQUEUE_TIMEOUT_US = 10_000L // 10ms codec dequeue wait.
    private const val STATE_PUSH_INTERVAL_MS = 100L // ~10Hz transport-state push.

    private class Session(
        val token: Long,
        val loop: Boolean,
        val maxDimension: Int,
    ) {
        var videoThread: HandlerThread? = null
        var audioThread: HandlerThread? = null
        var videoHandler: Handler? = null
        var audioHandler: Handler? = null

        @Volatile var playing = false
        @Volatile var muted = false
        @Volatile var rate = 1.0f
        // Closing flag the decode loops poll to exit promptly on teardown.
        val closing = AtomicBoolean(false)

        // Seek request (seconds), consumed by the video loop. NaN = none.
        @Volatile var seekRequestSec = Float.NaN

        @Volatile var durationSec = 0.0f
        @Volatile var positionSec = 0.0f

        var imageReader: ImageReader? = null
        // Reused direct output buffers — allocated once (resized only if the
        // frame dimensions change), so a frame neither allocates a Java array
        // nor copies across JNI (Rust reads the off-heap region zero-copy).
        var videoDirectBuf: ByteBuffer? = null
        var audioDirectBuf: ByteBuffer? = null
    }

    private val sessions = ConcurrentHashMap<Long, Session>()

    @JvmStatic
    fun open(
        context: Context,
        url: String,
        autoplay: Boolean,
        loopPlayback: Boolean,
        muted: Boolean,
        maxDimension: Int,
        token: Long,
    ) {
        // Run setup off the caller thread; decode is blocking.
        val setupThread = HandlerThread("idealyst-vdec-setup-$token").also { it.start() }
        Handler(setupThread.looper).post {
            try {
                start(context, url, autoplay, loopPlayback, muted, maxDimension, token)
            } catch (t: Throwable) {
                cleanup(token)
                nativeError(token, ERR_EXCEPTION, t.message ?: t.toString())
            } finally {
                setupThread.quitSafely()
            }
        }
    }

    @JvmStatic
    fun close(token: Long) {
        // Signal the loops to stop, then tear down on a worker (join blocks).
        sessions[token]?.closing?.set(true)
        val t = HandlerThread("idealyst-vdec-close-$token").also { it.start() }
        Handler(t.looper).post {
            cleanup(token)
            t.quitSafely()
        }
    }

    @JvmStatic fun play(token: Long) {
        sessions[token]?.let { it.playing = true; pushState(it) }
    }

    @JvmStatic fun pause(token: Long) {
        sessions[token]?.let { it.playing = false; pushState(it) }
    }

    @JvmStatic fun seek(token: Long, seconds: Float) {
        sessions[token]?.let { it.seekRequestSec = seconds }
    }

    @JvmStatic fun setMuted(token: Long, muted: Boolean) {
        sessions[token]?.let { it.muted = muted; pushState(it) }
    }

    @JvmStatic fun setRate(token: Long, rate: Float) {
        sessions[token]?.let {
            it.rate = if (rate < 0f) 0f else rate
            // rate 0 == paused (matches the trait contract).
            if (rate <= 0f) it.playing = false
            pushState(it)
        }
    }

    private fun start(
        context: Context,
        url: String,
        autoplay: Boolean,
        loopPlayback: Boolean,
        muted: Boolean,
        maxDimension: Int,
        token: Long,
    ) {
        val session = Session(token, loopPlayback, maxDimension)
        session.muted = muted
        session.playing = autoplay
        sessions[token] = session

        // --- Demux: locate the video + (optional) audio tracks. --------------
        val videoExtractor = MediaExtractor()
        setExtractorSource(videoExtractor, context, url)

        var videoTrack = -1
        var audioTrack = -1
        var videoFormat: MediaFormat? = null
        var audioFormat: MediaFormat? = null
        for (i in 0 until videoExtractor.trackCount) {
            val fmt = videoExtractor.getTrackFormat(i)
            val mime = fmt.getString(MediaFormat.KEY_MIME) ?: continue
            if (videoTrack < 0 && mime.startsWith("video/")) {
                videoTrack = i; videoFormat = fmt
            } else if (audioTrack < 0 && mime.startsWith("audio/")) {
                audioTrack = i; audioFormat = fmt
            }
        }

        if (videoTrack < 0 || videoFormat == null) {
            videoExtractor.release()
            nativeError(token, ERR_BAD_SOURCE, "no video track in $url")
            return
        }

        val natW = videoFormat.getInteger(MediaFormat.KEY_WIDTH)
        val natH = videoFormat.getInteger(MediaFormat.KEY_HEIGHT)
        // Honor max_dimension: cap the longest side, preserving aspect. The
        // ImageReader is allocated at the OUTPUT size and the codec is asked to
        // decode to it where supported; otherwise we downscale in conversion.
        val (outW, outH) = targetSize(natW, natH, maxDimension)

        session.durationSec = durationSeconds(videoFormat)
        val hasAudio = audioTrack >= 0

        // --- Video codec → ImageReader(RGBA_8888) surface. -------------------
        videoExtractor.selectTrack(videoTrack)
        // TODO(device): some devices reject a non-native ImageReader size for the
        // decoder surface; if frames come back at natW/natH regardless, the
        // conversion path below already handles arbitrary input dims, but the
        // ImageReader must then be allocated at natW/natH. Verify on hardware and
        // fall back to (natW,natH) for the reader if configure throws.
        val reader = ImageReader.newInstance(outW, outH, PixelFormat.RGBA_8888, 3)
        session.imageReader = reader

        val videoCodec = MediaCodec.createDecoderByType(
            videoFormat.getString(MediaFormat.KEY_MIME)!!
        )
        // Request the decode output size where the codec honors it.
        videoFormat.setInteger(MediaFormat.KEY_WIDTH, outW)
        videoFormat.setInteger(MediaFormat.KEY_HEIGHT, outH)
        videoCodec.configure(videoFormat, reader.surface, null, 0)

        // --- Audio codec (decode to PCM16 buffers) if present. ---------------
        var audioExtractor: MediaExtractor? = null
        var audioCodec: MediaCodec? = null
        if (hasAudio && audioFormat != null) {
            audioExtractor = MediaExtractor()
            setExtractorSource(audioExtractor, context, url)
            audioExtractor.selectTrack(audioTrack)
            audioCodec = MediaCodec.createDecoderByType(
                audioFormat.getString(MediaFormat.KEY_MIME)!!
            )
            audioCodec.configure(audioFormat, null, null, 0)
        }

        // Spin up worker threads BEFORE reporting open, so a frame can't race a
        // not-yet-registered session (Rust parks the writers before calling).
        val vThread = HandlerThread("idealyst-vdec-video-$token").also { it.start() }
        session.videoThread = vThread
        session.videoHandler = Handler(vThread.looper)

        // Report open up to Rust (resolves the awaiting open() future).
        nativeOpened(token, hasAudio, natW, natH)
        pushState(session)

        // Start the video decode loop on its thread.
        val readerForLoop = reader
        session.videoHandler!!.post {
            try {
                runVideoLoop(session, videoExtractor, videoCodec, readerForLoop, outW, outH)
            } catch (t: Throwable) {
                // Decode-time failure after open: nothing to resolve, just stop.
                // TODO(device): consider surfacing a runtime decode error channel.
            } finally {
                safeStopCodec(videoCodec)
                try { videoExtractor.release() } catch (_: Throwable) {}
            }
        }

        if (audioCodec != null && audioExtractor != null) {
            val aThread = HandlerThread("idealyst-vdec-audio-$token").also { it.start() }
            session.audioThread = aThread
            session.audioHandler = Handler(aThread.looper)
            val ae = audioExtractor
            val ac = audioCodec
            session.audioHandler!!.post {
                try {
                    runAudioLoop(session, ae, ac)
                } catch (_: Throwable) {
                } finally {
                    safeStopCodec(ac)
                    try { ae.release() } catch (_: Throwable) {}
                }
            }
        }
    }

    // ------------------------------------------------------------------------
    // Video decode loop: extractor → codec → ImageReader → RGBA → JNI.
    // ------------------------------------------------------------------------

    private fun runVideoLoop(
        session: Session,
        extractor: MediaExtractor,
        codec: MediaCodec,
        reader: ImageReader,
        outW: Int,
        outH: Int,
    ) {
        codec.start()
        val bufferInfo = MediaCodec.BufferInfo()
        var inputDone = false
        var outputDone = false

        // Playback clock anchor: maps decoded PTS to wall time so we render at
        // the clip's natural cadence. Re-anchored on (re)start, seek, and rate
        // change. `anchorWallNs`/`anchorPtsUs` define the line pts→wall.
        var anchorWallNs = SystemClock.elapsedRealtimeNanos()
        var anchorPtsUs = 0L
        var lastStatePushMs = 0L

        // ImageReader delivers frames to the listener; we render the codec
        // output buffer (which feeds the reader) then acquire+convert here.
        // Using acquireLatestImage in a poll keeps the loop self-paced.

        while (!session.closing.get() && !outputDone) {
            // Honor a pending seek: flush codecs + reposition the extractor.
            val seekTo = session.seekRequestSec
            if (!seekTo.isNaN()) {
                session.seekRequestSec = Float.NaN
                val us = (seekTo.coerceAtLeast(0f) * 1_000_000f).toLong()
                extractor.seekTo(us, MediaExtractor.SEEK_TO_PREVIOUS_SYNC)
                codec.flush()
                codec.start()
                inputDone = false
                outputDone = false
                anchorWallNs = SystemClock.elapsedRealtimeNanos()
                anchorPtsUs = us
                session.positionSec = seekTo
            }

            if (!session.playing) {
                // Paused: don't advance the clock; re-anchor so resume is smooth.
                Thread.sleep(15)
                anchorWallNs = SystemClock.elapsedRealtimeNanos()
                anchorPtsUs = (session.positionSec * 1_000_000f).toLong()
                maybePushState(session)
                continue
            }

            // Feed input.
            if (!inputDone) {
                val inIndex = codec.dequeueInputBuffer(DEQUEUE_TIMEOUT_US)
                if (inIndex >= 0) {
                    val inBuf = codec.getInputBuffer(inIndex)!!
                    val sampleSize = extractor.readSampleData(inBuf, 0)
                    if (sampleSize < 0) {
                        codec.queueInputBuffer(
                            inIndex, 0, 0, 0L,
                            MediaCodec.BUFFER_FLAG_END_OF_STREAM,
                        )
                        inputDone = true
                    } else {
                        codec.queueInputBuffer(
                            inIndex, 0, sampleSize, extractor.sampleTime, 0,
                        )
                        extractor.advance()
                    }
                }
            }

            // Drain output → render to the reader surface at the right time.
            val outIndex = codec.dequeueOutputBuffer(bufferInfo, DEQUEUE_TIMEOUT_US)
            when {
                outIndex >= 0 -> {
                    if (bufferInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) {
                        outputDone = true
                    }
                    val ptsUs = bufferInfo.presentationTimeUs
                    // Throttle to the clip's cadence: sleep until this frame's
                    // wall-time, scaled by playback rate.
                    val rate = if (session.rate <= 0f) 1f else session.rate
                    val targetWallNs =
                        anchorWallNs + ((ptsUs - anchorPtsUs) * 1000.0 / rate).toLong()
                    val sleepMs = (targetWallNs - SystemClock.elapsedRealtimeNanos()) / 1_000_000
                    if (sleepMs in 1..500) {
                        try { Thread.sleep(sleepMs) } catch (_: InterruptedException) {}
                    }
                    // releaseOutputBuffer(render=true) pushes the frame onto the
                    // ImageReader surface; we then acquire + convert it.
                    val render = bufferInfo.size > 0
                    codec.releaseOutputBuffer(outIndex, render)
                    if (render) {
                        drainReader(session, reader, ptsUs, outW, outH)
                    }
                    session.positionSec = ptsUs / 1_000_000f
                    maybePushState(session)
                }
                outIndex == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                    // Output format settled (size/colorspace). Reader already
                    // sized; nothing to do here for the surface path.
                }
                else -> {
                    // INFO_TRY_AGAIN_LATER: nothing ready yet.
                }
            }

            // Loop-at-end.
            if (outputDone && session.loop && !session.closing.get()) {
                extractor.seekTo(0L, MediaExtractor.SEEK_TO_CLOSEST_SYNC)
                codec.flush()
                codec.start()
                inputDone = false
                outputDone = false
                anchorWallNs = SystemClock.elapsedRealtimeNanos()
                anchorPtsUs = 0L
                session.positionSec = 0f
            } else if (outputDone) {
                session.playing = false
                pushState(session)
            }
        }
    }

    /**
     * Acquire the latest image the codec just rendered onto the reader surface,
     * convert it to tightly-packed top-down RGBA8 in [session]'s reused direct
     * buffer, and hand it to [nativeFrameDirect].
     */
    private fun drainReader(
        session: Session,
        reader: ImageReader,
        ptsUs: Long,
        outW: Int,
        outH: Int,
    ) {
        val image: Image = reader.acquireLatestImage() ?: return
        try {
            // RGBA_8888 from an ImageReader is a single plane with a row stride
            // that may exceed width*4 (alignment padding) — repack tightly.
            val w = image.width
            val h = image.height
            val needed = w * h * 4
            var buf = session.videoDirectBuf
            if (buf == null || buf.capacity() != needed) {
                buf = ByteBuffer.allocateDirect(needed).order(ByteOrder.nativeOrder())
                session.videoDirectBuf = buf
            }
            buf.clear()
            val plane = image.planes[0]
            val src = plane.buffer
            val rowStride = plane.rowStride
            val pixelStride = plane.pixelStride // 4 for RGBA_8888.
            val rowBytes = w * 4
            if (rowStride == rowBytes && pixelStride == 4) {
                // Tightly packed already — bulk copy each... actually whole.
                src.rewind()
                // Guard against a source shorter than expected.
                val copy = minOf(src.remaining(), needed)
                val tmpLimit = src.position() + copy
                src.limit(tmpLimit)
                buf.put(src)
            } else {
                // Strided / padded: copy row by row (and pixel by pixel if the
                // pixel stride isn't 4, though RGBA_8888 is always 4).
                val rowTmp = ByteArray(rowBytes)
                for (row in 0 until h) {
                    src.position(row * rowStride)
                    if (pixelStride == 4) {
                        val avail = minOf(src.remaining(), rowBytes)
                        src.get(rowTmp, 0, avail)
                        buf.put(rowTmp, 0, avail)
                    } else {
                        for (col in 0 until w) {
                            val base = row * rowStride + col * pixelStride
                            buf.put(src.get(base))
                            buf.put(src.get(base + 1))
                            buf.put(src.get(base + 2))
                            buf.put(src.get(base + 3))
                        }
                    }
                }
            }
            nativeFrameDirect(session.token, buf, w, h, ptsUs)
        } catch (_: Throwable) {
            // Drop the frame; never throw into the decode loop.
        } finally {
            image.close()
        }
    }

    // ------------------------------------------------------------------------
    // Audio decode loop: extractor → codec → PCM16 → interleaved f32 → JNI.
    // ------------------------------------------------------------------------

    private fun runAudioLoop(
        session: Session,
        extractor: MediaExtractor,
        codec: MediaCodec,
    ) {
        codec.start()
        val bufferInfo = MediaCodec.BufferInfo()
        var inputDone = false
        var outputDone = false
        var sampleRate = 0
        var channels = 0
        var lastSeek = Float.NaN

        while (!session.closing.get() && !outputDone) {
            // Audio follows the video loop's seek (it reads the same volatile).
            val seekTo = session.seekRequestSec
            if (!seekTo.isNaN() && seekTo != lastSeek) {
                lastSeek = seekTo
                val us = (seekTo.coerceAtLeast(0f) * 1_000_000f).toLong()
                extractor.seekTo(us, MediaExtractor.SEEK_TO_PREVIOUS_SYNC)
                codec.flush()
                codec.start()
                inputDone = false
                outputDone = false
            }
            if (!session.playing) {
                Thread.sleep(15)
                continue
            }

            if (!inputDone) {
                val inIndex = codec.dequeueInputBuffer(DEQUEUE_TIMEOUT_US)
                if (inIndex >= 0) {
                    val inBuf = codec.getInputBuffer(inIndex)!!
                    val sampleSize = extractor.readSampleData(inBuf, 0)
                    if (sampleSize < 0) {
                        codec.queueInputBuffer(
                            inIndex, 0, 0, 0L,
                            MediaCodec.BUFFER_FLAG_END_OF_STREAM,
                        )
                        inputDone = true
                    } else {
                        codec.queueInputBuffer(
                            inIndex, 0, sampleSize, extractor.sampleTime, 0,
                        )
                        extractor.advance()
                    }
                }
            }

            val outIndex = codec.dequeueOutputBuffer(bufferInfo, DEQUEUE_TIMEOUT_US)
            when {
                outIndex >= 0 -> {
                    if (bufferInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) {
                        outputDone = true
                    }
                    if (bufferInfo.size > 0) {
                        val outBuf = codec.getOutputBuffer(outIndex)!!
                        // Resolve the output format lazily (it can change after
                        // the first decoded buffer).
                        val ofmt = codec.outputFormat
                        sampleRate = ofmt.getInteger(MediaFormat.KEY_SAMPLE_RATE)
                        channels = ofmt.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
                        // TODO(device): MediaCodec audio output is PCM16
                        // (KEY_PCM_ENCODING == ENCODING_PCM_16BIT) on the
                        // overwhelming majority of devices, but some may emit
                        // float or 8-bit. Check KEY_PCM_ENCODING and branch.
                        emitPcm16(session, outBuf, bufferInfo, sampleRate, channels)
                    }
                    codec.releaseOutputBuffer(outIndex, false)
                }
                outIndex == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                    val ofmt = codec.outputFormat
                    sampleRate = ofmt.getInteger(MediaFormat.KEY_SAMPLE_RATE)
                    channels = ofmt.getInteger(MediaFormat.KEY_CHANNEL_COUNT)
                }
                else -> { /* try again later */ }
            }

            if (outputDone && session.loop && !session.closing.get()) {
                extractor.seekTo(0L, MediaExtractor.SEEK_TO_CLOSEST_SYNC)
                codec.flush()
                codec.start()
                inputDone = false
                outputDone = false
            }
        }
    }

    /**
     * Convert a PCM16 output buffer to interleaved f32 in [session]'s reused
     * direct buffer and hand it to [nativeAudio]. NOTE: the recorder's audio tap
     * is non-destructive on Apple; here the SDK doesn't drive an `AudioTrack`, so
     * "muted" only affects whether we'd play sound — playback audio output is a
     * device-test follow-up (see report). The PCM always flows for the mux.
     */
    private fun emitPcm16(
        session: Session,
        outBuf: ByteBuffer,
        info: MediaCodec.BufferInfo,
        sampleRate: Int,
        channels: Int,
    ) {
        if (sampleRate <= 0 || channels <= 0) return
        outBuf.position(info.offset)
        outBuf.limit(info.offset + info.size)
        val shorts = outBuf.order(ByteOrder.nativeOrder()).asShortBuffer()
        val sampleCount = shorts.remaining()
        if (sampleCount <= 0) return
        val frames = sampleCount / channels
        if (frames <= 0) return

        val neededBytes = sampleCount * 4
        var buf = session.audioDirectBuf
        if (buf == null || buf.capacity() < neededBytes) {
            buf = ByteBuffer.allocateDirect(neededBytes).order(ByteOrder.nativeOrder())
            session.audioDirectBuf = buf
        }
        buf.clear()
        val floats = buf.asFloatBuffer()
        val inv = 1.0f / 32768.0f
        for (i in 0 until sampleCount) {
            floats.put(shorts.get().toFloat() * inv)
        }
        nativeAudio(session.token, buf, sampleRate, channels, frames, info.presentationTimeUs)
    }

    // ------------------------------------------------------------------------
    // Helpers.
    // ------------------------------------------------------------------------

    private fun setExtractorSource(extractor: MediaExtractor, context: Context, url: String) {
        // file:// / content:// / http(s):// — let MediaExtractor resolve via Uri;
        // bare paths are treated as file paths.
        if (url.startsWith("/")) {
            extractor.setDataSource(url)
        } else {
            extractor.setDataSource(context, Uri.parse(url), null)
        }
    }

    private fun durationSeconds(format: MediaFormat): Float {
        return if (format.containsKey(MediaFormat.KEY_DURATION)) {
            format.getLong(MediaFormat.KEY_DURATION) / 1_000_000f
        } else {
            0f
        }
    }

    /** Cap the longest side to [maxDimension] (0 = no constraint), keeping aspect. */
    private fun targetSize(natW: Int, natH: Int, maxDimension: Int): Pair<Int, Int> {
        if (maxDimension <= 0 || natW <= 0 || natH <= 0) return Pair(natW, natH)
        val longest = maxOf(natW, natH)
        if (longest <= maxDimension) return Pair(natW, natH)
        val scale = maxDimension.toFloat() / longest
        val w = (natW * scale).toInt().coerceAtLeast(1)
        val h = (natH * scale).toInt().coerceAtLeast(1)
        // Codec output dimensions are best kept even.
        return Pair(w and 1.inv(), h and 1.inv())
    }

    private fun maybePushState(session: Session) {
        // Lightweight: the loops call this often; throttle inside.
        val now = SystemClock.uptimeMillis()
        val last = stateLastPush.getOrDefault(session.token, 0L)
        if (now - last >= STATE_PUSH_INTERVAL_MS) {
            stateLastPush[session.token] = now
            pushState(session)
        }
    }

    private val stateLastPush = ConcurrentHashMap<Long, Long>()

    private fun pushState(session: Session) {
        nativeState(
            session.token,
            session.positionSec,
            session.durationSec,
            session.playing,
            session.muted,
        )
    }

    private fun safeStopCodec(codec: MediaCodec?) {
        if (codec == null) return
        try { codec.stop() } catch (_: Throwable) {}
        try { codec.release() } catch (_: Throwable) {}
    }

    /** Tear down everything associated with [token]. Idempotent. */
    private fun cleanup(token: Long) {
        val session = sessions.remove(token) ?: return
        session.closing.set(true)
        // Stop the decode threads and WAIT for them to drain BEFORE releasing
        // the ImageReader — a frame may be mid-conversion reading the reader's
        // native Image; releasing it underneath is a use-after-free (the analog
        // of the camera SDK's "crashes a while after start" bug).
        session.videoThread?.let { t ->
            t.quitSafely()
            try { t.join(500) } catch (_: InterruptedException) {}
        }
        session.audioThread?.let { t ->
            t.quitSafely()
            try { t.join(500) } catch (_: InterruptedException) {}
        }
        try { session.imageReader?.close() } catch (_: Throwable) {}
        stateLastPush.remove(token)
    }

    @JvmStatic
    private external fun nativeOpened(token: Long, hasAudio: Boolean, width: Int, height: Int)

    @JvmStatic
    private external fun nativeError(token: Long, code: Int, message: String?)

    @JvmStatic
    private external fun nativeFrameDirect(
        token: Long,
        buffer: ByteBuffer,
        width: Int,
        height: Int,
        ptsMicros: Long,
    )

    @JvmStatic
    private external fun nativeAudio(
        token: Long,
        buffer: ByteBuffer,
        sampleRate: Int,
        channels: Int,
        frames: Int,
        ptsMicros: Long,
    )

    @JvmStatic
    private external fun nativeState(
        token: Long,
        position: Float,
        duration: Float,
        playing: Boolean,
        muted: Boolean,
    )
}
