package io.idealyst.keepawake

import android.app.Activity
import android.app.Application
import android.content.Context
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.WindowManager
import java.util.Collections
import java.util.WeakHashMap

/**
 * Puts `FLAG_KEEP_SCREEN_ON` on the windows of the app's Activities.
 *
 * The flag belongs to a window, and Android may destroy an Activity and make
 * another under a live hold, so this follows every Activity from the first
 * hold on: a new one gets the current state as it is created. Everything runs
 * on the main thread, where window flags must be set.
 */
object RustKeepAwake : Application.ActivityLifecycleCallbacks {
    private val main = Handler(Looper.getMainLooper())
    private val activities: MutableSet<Activity> =
        Collections.newSetFromMap(WeakHashMap<Activity, Boolean>())
    private var following = false
    private var on = false

    /** Called from Rust with the Context the host publishes. */
    @JvmStatic
    fun set(context: Context, on: Boolean) {
        val run = Runnable { apply(context, on) }
        if (Looper.myLooper() == Looper.getMainLooper()) run.run() else main.post(run)
    }

    private fun apply(context: Context, on: Boolean) {
        if (!following) {
            (context.applicationContext as? Application)?.let {
                it.registerActivityLifecycleCallbacks(this)
                following = true
            }
        }
        // The Activity made before the callbacks were registered.
        (context as? Activity)?.let { if (!it.isDestroyed) activities.add(it) }
        this.on = on
        activities.forEach { flag(it) }
    }

    private fun flag(activity: Activity) {
        if (on) {
            activity.window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        } else {
            activity.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
    }

    override fun onActivityCreated(activity: Activity, state: Bundle?) {
        activities.add(activity)
        if (on) flag(activity)
    }

    override fun onActivityDestroyed(activity: Activity) {
        activities.remove(activity)
    }

    override fun onActivityStarted(activity: Activity) {}
    override fun onActivityResumed(activity: Activity) {}
    override fun onActivityPaused(activity: Activity) {}
    override fun onActivityStopped(activity: Activity) {}
    override fun onActivitySaveInstanceState(activity: Activity, state: Bundle) {}
}
