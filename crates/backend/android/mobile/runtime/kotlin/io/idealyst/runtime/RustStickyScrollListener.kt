package io.idealyst.runtime

import android.view.View

/**
 * `View.OnScrollChangeListener` whose `onScrollChange` dispatches
 * into Rust via a cached scroll-view key.
 *
 * The backend's sticky registry installs one of these on each
 * ScrollView that has at least one `Position::Sticky` child. The
 * `scrollKey` carries the JObject raw pointer of the ScrollView's
 * GlobalRef — the same key the registry uses on the Rust side to
 * find the matching `StickyScrollEntry`. On every scroll change
 * we trampoline into `nativeOnScrollChange` with the live scroll
 * position so the Rust side can recompute each sticky child's
 * translation and apply it.
 *
 * Lifetime: held alive by a GlobalRef in `StickyScrollEntry`.
 * When the last sticky child deregisters from this ScrollView, the
 * Rust side calls `setOnScrollChangeListener(null)` to detach this
 * listener and drops the GlobalRef.
 */
class RustStickyScrollListener(private val scrollKey: Long) :
    View.OnScrollChangeListener {

    override fun onScrollChange(
        v: View,
        scrollX: Int,
        scrollY: Int,
        oldScrollX: Int,
        oldScrollY: Int,
    ) {
        // Android delivers scroll positions in device pixels; the
        // Rust side converts to dp using the scroll view's
        // display density (read via JNI). No conversion here.
        nativeOnScrollChange(scrollKey, scrollX.toFloat(), scrollY.toFloat())
    }

    private external fun nativeOnScrollChange(
        scrollKey: Long,
        scrollX: Float,
        scrollY: Float,
    )
}
