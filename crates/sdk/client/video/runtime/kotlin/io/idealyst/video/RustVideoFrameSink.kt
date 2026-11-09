package io.idealyst.video

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Matrix
import android.graphics.SurfaceTexture
import android.hardware.display.DisplayManager
import android.media.MediaPlayer
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.Display
import android.view.Surface
import android.view.TextureView
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.VideoView
import java.nio.ByteBuffer

/**
 * A [VideoView] that can FILL its bounds instead of aspect-fitting.
 *
 * Android's stock `VideoView.onMeasure` sizes the view to the video's aspect
 * ratio and paints the surrounding letterbox BLACK — so `object-fit: cover`
 * (and the case where the host box is already sized to the video's aspect)
 * shows an ugly black bar. With [fillMode] on, we measure to the full given
 * bounds so the video stretches to fill — no black bars. The caller sizes the
 * host box to the video's aspect, so there's no visible distortion.
 */
private class FillVideoView(context: Context) : VideoView(context) {
    var fillMode = false

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        if (fillMode) {
            setMeasuredDimension(
                getDefaultSize(suggestedMinimumWidth, widthMeasureSpec),
                getDefaultSize(suggestedMinimumHeight, heightMeasureSpec),
            )
        } else {
            super.onMeasure(widthMeasureSpec, heightMeasureSpec)
        }
    }
}

/**
 * Display helper for the `video` SDK on Android.
 *
 * The `video` external mounts a [FrameLayout] host. A URL source gets a
 * [VideoView] child. A live `MediaStream` gets a [TextureView] child the capture
 * fills directly when the stream has a preview source and this display holds it
 * ([showPreview]), and otherwise an [ImageView] child whose `Bitmap` is replaced
 * each frame from tightly-packed RGBA8 bytes ([showFrame]). The Rust side calls
 * these on the **UI thread** (the framework's frame loop runs on the main looper),
 * so no `Handler.post` is needed here.
 *
 * Children are tagged so they can be found / reused across frames. Shipped
 * from the `video` crate via `[package.metadata.idealyst.android].runtime_kotlin`.
 */
object RustVideoFrameSink {
    private const val IMAGE_TAG = "idealyst_stream_image"
    private const val VIDEO_TAG = "idealyst_video_view"
    private const val PREVIEW_TAG = "idealyst_stream_preview"

    private const val TAG = "idealyst-video"

    /**
     * How long a shown preview waits for its first frame before giving up on the
     * native path. Generous: the first frame on a slow tablet can follow a session
     * rebuild, and giving up costs the preview its frame rate.
     */
    private const val FIRST_FRAME_TIMEOUT_MS = 4000L

    /**
     * How long a surface outlives its view. A display's detach reaches the camera
     * asynchronously and the frames already in flight still land in the surface; a
     * surface released under them makes the camera drop buffers and fail requests.
     * Longest pipeline Camera2 allows, at a slow frame rate, is well inside this.
     */
    private const val SURFACE_RELEASE_DELAY_MS = 1000L

    private fun matchParent() =
        FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT,
        )

    /**
     * A [TextureView] showing a camera preview, keeping its transform current and
     * telling Rust about its surface.
     *
     * `TextureView` rather than `SurfaceView`, so it composites inside the View tree:
     * a rounded parent clips it, and siblings draw over it. A `SurfaceView` is a hole
     * through the window, which neither clips nor layers with the rest of the screen.
     */
    private class Preview(
        val view: TextureView,
        val id: Long,
        val bufferWidth: Int,
        val bufferHeight: Int,
        val sensorOrientation: Int,
        val front: Boolean,
        val cover: Boolean,
    ) : TextureView.SurfaceTextureListener, View.OnLayoutChangeListener,
        DisplayManager.DisplayListener {
        private val main = Handler(Looper.getMainLooper())
        private val displays =
            view.context.getSystemService(Context.DISPLAY_SERVICE) as? DisplayManager
        private var surface: Surface? = null
        private var sawFrame = false
        private var removed = false

        private val firstFrameCheck = object : Runnable {
            override fun run() {
                if (removed || sawFrame) return
                // Not on screen, so no frame is expected yet. A view that is never
                // drawn never gets a surface either, which is the case this catches
                // when it IS on screen: a window without hardware acceleration.
                val onScreen = view.isShown && view.width > 0 && view.height > 0 &&
                    view.windowVisibility == View.VISIBLE
                if (!onScreen) {
                    main.postDelayed(this, FIRST_FRAME_TIMEOUT_MS)
                    return
                }
                Log.w(TAG, "preview: no frame in ${FIRST_FRAME_TIMEOUT_MS}ms, using CPU frames")
                nativePreviewStalled(id)
            }
        }

        fun start() {
            view.surfaceTextureListener = this
            view.addOnLayoutChangeListener(this)
            displays?.registerDisplayListener(this, main)
            main.postDelayed(firstFrameCheck, FIRST_FRAME_TIMEOUT_MS)
        }

        /** The view is coming off the host. Its surface callback follows if it has one. */
        fun stop() {
            removed = true
            main.removeCallbacks(firstFrameCheck)
            view.removeOnLayoutChangeListener(this)
            displays?.unregisterDisplayListener(this)
        }

        private fun applyTransform() {
            val width = view.width
            val height = view.height
            if (width == 0 || height == 0) return
            val rotation = displays?.getDisplay(Display.DEFAULT_DISPLAY)?.rotation ?: 0
            val t = nativePreviewTransform(
                bufferWidth, bufferHeight, sensorOrientation, front, rotation, width, height, cover,
            )
            val (cx, cy) = width / 2f to height / 2f
            val matrix = Matrix()
            matrix.setScale(t[0], t[1], cx, cy)
            matrix.postRotate(t[2], cx, cy)
            view.setTransform(matrix)
        }

        override fun onSurfaceTextureAvailable(texture: SurfaceTexture, width: Int, height: Int) {
            // The view just set the buffer size to its own; the camera's output is
            // fixed at this one.
            texture.setDefaultBufferSize(bufferWidth, bufferHeight)
            val created = Surface(texture)
            surface = created
            applyTransform()
            Log.i(TAG, "preview: surface ready, ${bufferWidth}x$bufferHeight in ${width}x$height")
            nativePreviewSurfaceAvailable(id, created)
        }

        override fun onSurfaceTextureSizeChanged(texture: SurfaceTexture, width: Int, height: Int) {
            texture.setDefaultBufferSize(bufferWidth, bufferHeight)
            applyTransform()
        }

        override fun onSurfaceTextureDestroyed(texture: SurfaceTexture): Boolean {
            val going = surface
            surface = null
            nativePreviewSurfaceDestroyed(id)
            // Returning false keeps the texture ours to release: see
            // SURFACE_RELEASE_DELAY_MS.
            main.postDelayed({
                going?.release()
                texture.release()
            }, SURFACE_RELEASE_DELAY_MS)
            return false
        }

        override fun onSurfaceTextureUpdated(texture: SurfaceTexture) {
            if (!sawFrame) {
                sawFrame = true
                main.removeCallbacks(firstFrameCheck)
                Log.i(TAG, "preview: first frame")
                // `removed`: a view on its way off the host no longer speaks for
                // the display (its id is already gone on the Rust side too).
                if (!removed) nativePreviewFirstFrame(id)
            }
        }

        override fun onLayoutChange(
            v: View,
            left: Int,
            top: Int,
            right: Int,
            bottom: Int,
            oldLeft: Int,
            oldTop: Int,
            oldRight: Int,
            oldBottom: Int,
        ) {
            if (right - left != oldRight - oldLeft || bottom - top != oldBottom - oldTop) {
                applyTransform()
            }
        }

        override fun onDisplayAdded(displayId: Int) {}
        override fun onDisplayRemoved(displayId: Int) {}

        /**
         * A rotation changes the transform, and a 180° turn changes nothing else: no
         * layout, no configuration. So this is a display listener, as the camera's is.
         */
        override fun onDisplayChanged(displayId: Int) {
            if (displayId == Display.DEFAULT_DISPLAY) applyTransform()
        }
    }

    /**
     * Show a capture's preview in the host: a [TextureView] whose surface goes to Rust
     * under [id] as soon as it exists, replacing the CPU path's [ImageView] if one was
     * showing. [bufferWidth] x [bufferHeight] is the size the capture writes, in its
     * sensor's orientation. Called once per preview; [hidePreview] ends it.
     */
    @JvmStatic
    fun showPreview(
        host: FrameLayout,
        id: Long,
        bufferWidth: Int,
        bufferHeight: Int,
        sensorOrientation: Int,
        front: Boolean,
        cover: Boolean,
    ) {
        hidePreview(host)
        host.findViewWithTag<View>(IMAGE_TAG)?.let { host.removeView(it) }
        val view = TextureView(host.context)
        view.tag = PREVIEW_TAG
        // A camera buffer has no alpha. Only a letterbox shows what is behind.
        view.isOpaque = cover
        val preview =
            Preview(view, id, bufferWidth, bufferHeight, sensorOrientation, front, cover)
        view.setTag(R_PREVIEW_STATE, preview)
        preview.start()
        host.addView(view, matchParent())
    }

    /** End the host's preview, if it has one. Removing the view ends its surface. */
    @JvmStatic
    fun hidePreview(host: FrameLayout) {
        val view = host.findViewWithTag<View>(PREVIEW_TAG) ?: return
        (view.getTag(R_PREVIEW_STATE) as? Preview)?.stop()
        host.removeView(view)
    }

    /**
     * The key [Preview] sits under on its view. An application-namespace id (top byte
     * 0x7f, type byte above 0x01), which is the only rule `View.setTag(int, Object)` has.
     */
    private const val R_PREVIEW_STATE = 0x7f7e0001

    /** Show one tightly-packed RGBA8 frame in the host's stream ImageView,
     *  creating the ImageView on first use. `rgba` is a direct ByteBuffer the
     *  Rust side hands over zero-copy (it views Rust-owned memory for the
     *  duration of this synchronous call); we copy out of it into the Bitmap
     *  and never retain it. The Bitmap is reused across frames when the
     *  dimensions match, so a steady stream allocates nothing per frame. */
    @JvmStatic
    fun showFrame(host: FrameLayout, rgba: ByteBuffer, width: Int, height: Int, cover: Boolean) {
        if (width <= 0 || height <= 0 || rgba.capacity() < width * height * 4) return
        // object-fit: Cover → CENTER_CROP (fill the box, crop overflow);
        // Contain → FIT_CENTER (letterbox). Matches the web/Apple mapping.
        val scale = if (cover) ImageView.ScaleType.CENTER_CROP else ImageView.ScaleType.FIT_CENTER
        var image = host.findViewWithTag<View>(IMAGE_TAG) as? ImageView
        if (image == null) {
            image = ImageView(host.context)
            image.tag = IMAGE_TAG
            host.addView(image, matchParent())
        }
        if (image.scaleType != scale) image.scaleType = scale
        val existing = (image.drawable as? android.graphics.drawable.BitmapDrawable)?.bitmap
        val reusable = existing != null && !existing.isRecycled &&
            existing.width == width && existing.height == height &&
            existing.config == Bitmap.Config.ARGB_8888
        // Android's ARGB_8888 is RGBA in memory order, matching our frames.
        val bitmap = if (reusable) existing!! else
            Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        rgba.rewind()
        bitmap.copyPixelsFromBuffer(rgba)
        if (reusable) {
            // Same Bitmap object already on the ImageView — mutated in place,
            // so force a redraw without rebuilding the drawable.
            image.invalidate()
        } else {
            image.setImageBitmap(bitmap)
        }
    }

    /** Get (creating if needed) the host's URL VideoView child. `fill` →
     *  object-fit: cover (stretch to fill the box, no black letterbox); else
     *  the stock aspect-fit (Contain) behavior. The view reports each clip's
     *  first rendered frame to Rust under [id]; VideoView hands the listener to
     *  every MediaPlayer it opens, so a new URI reports again. */
    @JvmStatic
    fun ensureVideoView(host: FrameLayout, fill: Boolean, id: Long): VideoView {
        var video = host.findViewWithTag<View>(VIDEO_TAG) as? FillVideoView
        if (video == null) {
            video = FillVideoView(host.context)
            video.tag = VIDEO_TAG
            video.setOnInfoListener { _, what, _ ->
                if (what == MediaPlayer.MEDIA_INFO_VIDEO_RENDERING_START) {
                    nativeVideoRenderingStart(id)
                }
                false
            }
            host.addView(video, matchParent())
        }
        if (video.fillMode != fill) {
            video.fillMode = fill
            video.requestLayout()
        }
        return video
    }

    /** The host's VideoView child if one exists, else null (for imperative ops). */
    @JvmStatic
    fun videoView(host: FrameLayout): VideoView? =
        host.findViewWithTag<View>(VIDEO_TAG) as? VideoView

    /** The preview's [surface] exists: hand it to the capture. */
    @JvmStatic
    private external fun nativePreviewSurfaceAvailable(id: Long, surface: Surface)

    /** The preview's surface is going away: stop the capture naming it. */
    @JvmStatic
    private external fun nativePreviewSurfaceDestroyed(id: Long)

    /** The preview's [TextureView] drew its first frame. */
    @JvmStatic
    private external fun nativePreviewFirstFrame(id: Long)

    /** The URL [VideoView] of the display registered under [id] rendered its first frame. */
    @JvmStatic
    private external fun nativeVideoRenderingStart(id: Long)

    /** The preview was on screen and no frame came: give up on the native path. */
    @JvmStatic
    private external fun nativePreviewStalled(id: Long)

    /**
     * `[scaleX, scaleY, rotationDegrees]` for this preview, applied about the view's
     * center. Implemented in Rust (`android_preview::preview_transform`).
     */
    @JvmStatic
    private external fun nativePreviewTransform(
        bufferWidth: Int,
        bufferHeight: Int,
        sensorOrientation: Int,
        front: Boolean,
        displayRotation: Int,
        viewWidth: Int,
        viewHeight: Int,
        cover: Boolean,
    ): FloatArray
}
