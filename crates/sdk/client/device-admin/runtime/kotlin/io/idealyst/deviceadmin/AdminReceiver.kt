package io.idealyst.deviceadmin

import android.app.admin.DeviceAdminReceiver
import android.app.admin.DevicePolicyManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.PersistableBundle
import org.json.JSONObject

/**
 * The device-admin receiver the device owner is bound through:
 * `adb shell dpm set-device-owner <app>/io.idealyst.deviceadmin.AdminReceiver`,
 * or the component a provisioning QR code names. Declared in the SDK's
 * `manifest/application.xml`, so every app that depends on the SDK has one;
 * it is inert until the app is made an admin.
 *
 * Every callback becomes an event through [RustDeviceAdmin.emit].
 */
class AdminReceiver : DeviceAdminReceiver() {
    override fun onEnabled(context: Context, intent: Intent) {
        RustDeviceAdmin.emit(context, JSONObject().put("type", "admin_enabled"))
    }

    override fun onDisabled(context: Context, intent: Intent) {
        RustDeviceAdmin.emit(context, JSONObject().put("type", "admin_disabled"))
    }

    override fun onLockTaskModeEntering(context: Context, intent: Intent, pkg: String) {
        RustDeviceAdmin.emit(context, JSONObject().put("type", "lock_task_entering").put("package", pkg))
    }

    override fun onLockTaskModeExiting(context: Context, intent: Intent) {
        RustDeviceAdmin.emit(context, JSONObject().put("type", "lock_task_exiting"))
    }

    override fun onProfileProvisioningComplete(context: Context, intent: Intent) {
        val extras: PersistableBundle? = if (Build.VERSION.SDK_INT >= 33) {
            intent.getParcelableExtra(DevicePolicyManager.EXTRA_PROVISIONING_ADMIN_EXTRAS_BUNDLE, PersistableBundle::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra(DevicePolicyManager.EXTRA_PROVISIONING_ADMIN_EXTRAS_BUNDLE)
        }
        RustDeviceAdmin.emitProvisioningComplete(context, extras)
    }
}
