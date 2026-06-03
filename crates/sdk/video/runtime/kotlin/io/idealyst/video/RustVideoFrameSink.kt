package io.idealyst.video

import android.graphics.Bitmap
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.VideoView
import java.nio.ByteBuffer

/**
 * Display helper for the `video` SDK on Android.
 *
 * The `video` external mounts a [FrameLayout] host. A URL source gets a
 * [VideoView] child; a live `MediaStream` source gets an [ImageView] child
 * whose `Bitmap` is replaced each frame from tightly-packed RGBA8 bytes. The
 * Rust side calls these on the **UI thread** (the framework's frame loop runs
 * on the main looper), so no `Handler.post` is needed here.
 *
 * Children are tagged so they can be found / reused across frames. Shipped
 * from the `video` crate via `[package.metadata.idealyst.android].runtime_kotlin`.
 */
object RustVideoFrameSink {
    private const val IMAGE_TAG = "idealyst_stream_image"
    private const val VIDEO_TAG = "idealyst_video_view"

    private fun matchParent() =
        FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT,
        )

    /** Show one tightly-packed RGBA8 frame in the host's stream ImageView,
     *  creating the ImageView on first use. */
    @JvmStatic
    fun showFrame(host: FrameLayout, rgba: ByteArray, width: Int, height: Int) {
        if (width <= 0 || height <= 0 || rgba.size < width * height * 4) return
        var image = host.findViewWithTag<View>(IMAGE_TAG) as? ImageView
        if (image == null) {
            image = ImageView(host.context)
            image.tag = IMAGE_TAG
            image.scaleType = ImageView.ScaleType.FIT_CENTER
            host.addView(image, matchParent())
        }
        // Android's ARGB_8888 is RGBA in memory order, matching our frames.
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        bitmap.copyPixelsFromBuffer(ByteBuffer.wrap(rgba))
        image.setImageBitmap(bitmap)
    }

    /** Get (creating if needed) the host's URL VideoView child. */
    @JvmStatic
    fun ensureVideoView(host: FrameLayout): VideoView {
        var video = host.findViewWithTag<View>(VIDEO_TAG) as? VideoView
        if (video == null) {
            video = VideoView(host.context)
            video.tag = VIDEO_TAG
            host.addView(video, matchParent())
        }
        return video
    }

    /** The host's VideoView child if one exists, else null (for imperative ops). */
    @JvmStatic
    fun videoView(host: FrameLayout): VideoView? =
        host.findViewWithTag<View>(VIDEO_TAG) as? VideoView
}
