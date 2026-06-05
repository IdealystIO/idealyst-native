package io.idealyst.video

import android.content.Context
import android.graphics.Bitmap
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
     *  the stock aspect-fit (Contain) behavior. */
    @JvmStatic
    fun ensureVideoView(host: FrameLayout, fill: Boolean): VideoView {
        var video = host.findViewWithTag<View>(VIDEO_TAG) as? FillVideoView
        if (video == null) {
            video = FillVideoView(host.context)
            video.tag = VIDEO_TAG
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
}
