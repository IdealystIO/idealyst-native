package io.idealyst.deviceadmin

import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.view.WindowManager

/**
 * Turns the screen on and goes away, for `device_admin::wake_screen`.
 *
 * Invisible and in a task of its own, so whatever was in front is in front
 * again when it finishes. A wake lock with `ACQUIRE_CAUSES_WAKEUP` is not
 * used: Android 15 ignores it without a permission a device owner cannot
 * hold.
 */
class WakeActivity : Activity() {
    private val main = Handler(Looper.getMainLooper())
    private val giveUp = Runnable { finish() }
    private var screenOn: BroadcastReceiver? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= 27) {
            setShowWhenLocked(true)
            setTurnScreenOn(true)
        } else {
            @Suppress("DEPRECATION")
            window.addFlags(
                WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED or WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON,
            )
        }
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) = finish()
        }
        RustDeviceAdmin.register(this, receiver, IntentFilter(Intent.ACTION_SCREEN_ON), exported = false)
        screenOn = receiver
        main.postDelayed(giveUp, GIVE_UP_MS)
    }

    override fun onResume() {
        super.onResume()
        // The screen was on by the time the window came up: there is no
        // ACTION_SCREEN_ON left to wait for.
        if ((getSystemService(Context.POWER_SERVICE) as PowerManager).isInteractive) finish()
    }

    override fun onDestroy() {
        main.removeCallbacks(giveUp)
        screenOn?.let { unregisterReceiver(it) }
        screenOn = null
        super.onDestroy()
    }

    companion object {
        /** A screen that has not come on by now is not going to; never stay over the kiosk. */
        private const val GIVE_UP_MS = 5_000L
    }
}
