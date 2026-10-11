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
 *
 * It holds no Rust pointer. Every registration is undone when the observed
 * Activity is destroyed, so a re-created app never hears from a stale one.
 * UI thread only (observers are bound to the main looper).
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

    fun attach(root: View) {
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
    }

    fun detach() {
        val r = root ?: return
        root = null
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
        val r = root ?: return
        val scale = Settings.Global.getFloat(
            r.context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f,
        )
        nativeAppAnimatorScale(scale)
    }

    private fun reportPointerDevices() {
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

    // --- window focus ---
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        if (root != null) nativeAppFocused(hasFocus)
    }

    // --- input devices ---
    override fun onInputDeviceAdded(deviceId: Int) = reportPointerDevices()
    override fun onInputDeviceRemoved(deviceId: Int) = reportPointerDevices()
    override fun onInputDeviceChanged(deviceId: Int) = reportPointerDevices()

    // --- Activity lifecycle (only the root's Activity) ---
    override fun onActivityStarted(a: Activity) {
        if (a === activity) nativeAppVisible(true)
    }

    override fun onActivityStopped(a: Activity) {
        if (a === activity) nativeAppVisible(false)
    }

    override fun onActivityDestroyed(a: Activity) {
        if (a === activity) detach()
    }

    override fun onActivityCreated(a: Activity, savedInstanceState: Bundle?) {}
    override fun onActivityResumed(a: Activity) {}
    override fun onActivityPaused(a: Activity) {}
    override fun onActivitySaveInstanceState(a: Activity, outState: Bundle) {}

    private external fun nativeAppFocused(focused: Boolean)
    private external fun nativeAppVisible(visible: Boolean)
    private external fun nativeAppAnimatorScale(scale: Float)
    private external fun nativeAppPointerDevices(hasMouse: Boolean, hasTouchscreen: Boolean)
}
