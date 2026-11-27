package io.idealyst.runtime

import android.app.Activity
import android.app.Application
import android.content.pm.PackageManager
import android.database.ContentObserver
import android.hardware.input.InputManager
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.view.InputDevice
import android.view.View
import android.view.ViewTreeObserver

/**
 * App-situation observer installed once by the Rust backend at boot
 * (`imp/app_state.rs`, from `AndroidBackend::new`). Reports into
 * `runtime_shared::app_state` through the `nativeApp*` trampolines in
 * `imp/jni_exports.rs`:
 *
 * - window focus (root `ViewTreeObserver`) → `nativeAppFocused`
 * - the root's Activity started / stopped (`ActivityLifecycleCallbacks`)
 *   → `nativeAppVisible`
 * - `Settings.Global.ANIMATOR_DURATION_SCALE` (+ `ContentObserver`; the
 *   "Remove animations" switch sets it to 0) → `nativeAppAnimatorScale`
 *   (Rust decides `== 0` → reduce motion)
 * - connected non-virtual `SOURCE_MOUSE` devices (+ `InputDeviceListener`)
 *   and `FEATURE_TOUCHSCREEN` → `nativeAppPointerDevices` (Rust maps to
 *   Fine / Coarse / None)
 * - the launch screen: `false` from [attach], `true` right after the root's
 *   first draw → `nativeAppSplashDismissed` (see [watchFirstDraw])
 *
 * It holds no Rust pointer. Every registration is undone when the observed
 * Activity is destroyed, so a re-created app never hears from a stale one.
 * UI thread only (observers are bound to the main looper).
 *
 * Only the newest observer reports ([current]). Android can create a new
 * Activity before the old one stops: the old Activity's `onStop` and
 * focus loss then arrive AFTER the new app attached and reported itself
 * visible, and would leave `app_visible()` / `app_focused()` false for an
 * app that is on screen.
 */
class RustAppStateObserver :
    ViewTreeObserver.OnWindowFocusChangeListener,
    Application.ActivityLifecycleCallbacks,
    InputManager.InputDeviceListener {

    private var root: View? = null
    private var observer: ViewTreeObserver? = null
    private var activity: Activity? = null
    private var inputManager: InputManager? = null
    private var motionObserver: ContentObserver? = null
    private var attachWatch: View.OnAttachStateChangeListener? = null
    private var drawWatch: ViewTreeObserver.OnDrawListener? = null

    fun attach(root: View) {
        current = this
        this.root = root
        val ctx = root.context
        val vto = root.viewTreeObserver
        vto.addOnWindowFocusChangeListener(this)
        observer = vto
        // At boot (Activity.onCreate) the root isn't attached yet and the
        // first focus callback reports; only read now if it already is.
        if (root.isAttachedToWindow) nativeAppFocused(root.hasWindowFocus())

        val act = RustGlobalPointerListener.findActivity(ctx)
        activity = act
        act?.application?.registerActivityLifecycleCallbacks(this)

        val main = Handler(Looper.getMainLooper())
        val resolver = ctx.contentResolver
        val mo = object : ContentObserver(main) {
            override fun onChange(selfChange: Boolean) = reportAnimatorScale()
        }
        resolver.registerContentObserver(
            Settings.Global.getUriFor(Settings.Global.ANIMATOR_DURATION_SCALE), false, mo,
        )
        motionObserver = mo
        reportAnimatorScale()

        val im = ctx.getSystemService(android.content.Context.INPUT_SERVICE) as? InputManager
        inputManager = im
        im?.registerInputDeviceListener(this, main)
        reportPointerDevices()

        watchFirstDraw(root, main)
    }

    /**
     * Report the launch screen as up, then as gone right after the root's
     * first draw: the frame the system splash (API 31+ `SplashScreen`, the
     * theme's `windowBackground` below) gives way to.
     *
     * The draw listener goes on the root's live observer once the root is
     * attached: before API 26 a detached view's floating `ViewTreeObserver`
     * drops `OnDrawListener`s when it merges into the window's. An
     * already-attached root is invalidated so a draw is guaranteed to come.
     * The report is posted at the front of the queue because
     * `removeOnDrawListener` throws inside `onDraw`, and the frame reaches
     * the RenderThread only after the draw pass returns.
     */
    private fun watchFirstDraw(root: View, main: Handler) {
        nativeAppSplashDismissed(false)
        val onDraw = object : ViewTreeObserver.OnDrawListener {
            private var fired = false
            override fun onDraw() {
                if (fired) return
                fired = true
                main.postAtFrontOfQueue {
                    removeDrawWatch(root)
                    if (reporting) nativeAppSplashDismissed(true)
                }
            }
        }
        drawWatch = onDraw
        if (root.isAttachedToWindow) {
            root.viewTreeObserver.addOnDrawListener(onDraw)
            root.invalidate()
            return
        }
        val onAttach = object : View.OnAttachStateChangeListener {
            override fun onViewAttachedToWindow(v: View) {
                v.removeOnAttachStateChangeListener(this)
                attachWatch = null
                v.viewTreeObserver.addOnDrawListener(onDraw)
            }

            override fun onViewDetachedFromWindow(v: View) {}
        }
        attachWatch = onAttach
        root.addOnAttachStateChangeListener(onAttach)
    }

    private fun removeDrawWatch(root: View) {
        attachWatch?.let { root.removeOnAttachStateChangeListener(it) }
        attachWatch = null
        drawWatch?.let { l ->
            val vto = root.viewTreeObserver
            if (vto.isAlive) vto.removeOnDrawListener(l)
        }
        drawWatch = null
    }

    fun detach() {
        val r = root ?: return
        removeDrawWatch(r)
        root = null
        if (current === this) current = null
        for (vto in listOfNotNull(observer, r.viewTreeObserver)) {
            if (vto.isAlive) vto.removeOnWindowFocusChangeListener(this)
        }
        observer = null
        activity?.application?.unregisterActivityLifecycleCallbacks(this)
        activity = null
        motionObserver?.let { r.context.contentResolver.unregisterContentObserver(it) }
        motionObserver = null
        inputManager?.unregisterInputDeviceListener(this)
        inputManager = null
    }

    private fun reportAnimatorScale() {
        if (current !== this) return
        val r = root ?: return
        val scale = Settings.Global.getFloat(
            r.context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f,
        )
        nativeAppAnimatorScale(scale)
    }

    private fun reportPointerDevices() {
        if (current !== this) return
        val r = root ?: return
        var hasMouse = false
        for (id in InputDevice.getDeviceIds()) {
            val dev = InputDevice.getDevice(id) ?: continue
            if (!dev.isVirtual && dev.supportsSource(InputDevice.SOURCE_MOUSE)) {
                hasMouse = true
                break
            }
        }
        val hasTouch = r.context.packageManager.hasSystemFeature(PackageManager.FEATURE_TOUCHSCREEN)
        nativeAppPointerDevices(hasMouse, hasTouch)
    }

    private val reporting get() = root != null && current === this

    // --- window focus ---
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        if (reporting) nativeAppFocused(hasFocus)
    }

    // --- input devices ---
    override fun onInputDeviceAdded(deviceId: Int) = reportPointerDevices()
    override fun onInputDeviceRemoved(deviceId: Int) = reportPointerDevices()
    override fun onInputDeviceChanged(deviceId: Int) = reportPointerDevices()

    // --- Activity lifecycle (only the root's Activity) ---
    override fun onActivityStarted(a: Activity) {
        if (a === activity && reporting) nativeAppVisible(true)
    }

    override fun onActivityStopped(a: Activity) {
        if (a === activity && reporting) nativeAppVisible(false)
    }

    override fun onActivityDestroyed(a: Activity) {
        if (a === activity) detach()
    }

    override fun onActivityCreated(a: Activity, savedInstanceState: Bundle?) {}
    override fun onActivityResumed(a: Activity) {}
    override fun onActivityPaused(a: Activity) {}
    override fun onActivitySaveInstanceState(a: Activity, outState: Bundle) {}

    companion object {
        /** The observer of the newest attach; the only one that reports. */
        private var current: RustAppStateObserver? = null
    }

    private external fun nativeAppFocused(focused: Boolean)
    private external fun nativeAppVisible(visible: Boolean)
    private external fun nativeAppAnimatorScale(scale: Float)
    private external fun nativeAppPointerDevices(hasMouse: Boolean, hasTouchscreen: Boolean)
    private external fun nativeAppSplashDismissed(dismissed: Boolean)
}
