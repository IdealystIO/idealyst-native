package io.idealyst.runtime

import android.content.Context
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback

/**
 * Routes the system Back (button and gesture: both go through the
 * Activity's `OnBackPressedDispatcher`) to the app's `on_back` handlers —
 * the Android side of `imp/system_back.rs`.
 *
 * Created by the Rust backend at attach, BEFORE the app tree mounts. The
 * dispatcher runs callbacks newest first, so every stack presenter the
 * tree adds later takes Back before this one: navigators pop their own
 * screens and keep a locked screen's Back, and only the Back that would
 * leave the app reaches the handlers.
 *
 * [callback] is enabled only while Rust reports a handler ([setWanted]).
 * An enabled callback turns off the system's predictive back-to-home
 * animation, so an app with no handler never pays for one.
 *
 * When no handler consumes a Back ([nativeDispatch] returns false) the
 * Activity gets the platform default: the callback steps aside and the
 * dispatcher runs again, reaching the Activity's own fallback (finish, or
 * move the task to the back).
 *
 * [dispose] removes the callback. Without it a handler-less app would keep
 * swallowing Back until the Activity died.
 */
class RustSystemBack(context: Context) {
    private val activity = RustGlobalPointerListener.findActivity(context) as? ComponentActivity

    /** What Rust last asked for; restored after a default re-dispatch. */
    private var wanted = false

    private val callback = object : OnBackPressedCallback(false) {
        override fun handleOnBackPressed() {
            if (nativeDispatch()) return
            isEnabled = false
            activity?.onBackPressedDispatcher?.onBackPressed()
            isEnabled = wanted
        }
    }

    init {
        activity?.onBackPressedDispatcher?.addCallback(callback)
    }

    fun setWanted(wanted: Boolean) {
        this.wanted = wanted
        callback.isEnabled = wanted
    }

    fun dispose() {
        wanted = false
        callback.isEnabled = false
        callback.remove()
    }

    private external fun nativeDispatch(): Boolean
}
