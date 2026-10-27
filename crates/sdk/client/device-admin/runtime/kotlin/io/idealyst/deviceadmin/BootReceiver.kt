package io.idealyst.deviceadmin

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import org.json.JSONObject

/**
 * `BOOT_COMPLETED`: reported as a `boot_completed` event (queued until the
 * app's Rust side listens — at boot the app's activity is usually not
 * running).
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED) {
            RustDeviceAdmin.emit(context, JSONObject().put("type", "boot_completed"))
        }
    }
}
