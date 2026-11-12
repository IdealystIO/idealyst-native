package io.idealyst.deviceadmin

import android.app.Activity
import android.app.admin.DevicePolicyManager
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.PersistableBundle

/**
 * The two activities the setup wizard requires of a device-owner app on
 * Android 10+ during QR provisioning. Neither shows UI.
 *
 * [ProvisioningModeActivity] answers `GET_PROVISIONING_MODE`: this app wants
 * to manage the whole device (fully managed), not a work profile.
 */
class ProvisioningModeActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val result = Intent()
        if (Build.VERSION.SDK_INT >= 29) {
            result.putExtra(
                DevicePolicyManager.EXTRA_PROVISIONING_MODE,
                DevicePolicyManager.PROVISIONING_MODE_FULLY_MANAGED_DEVICE,
            )
        }
        setResult(RESULT_OK, result)
        finish()
    }
}

/**
 * Answers `ADMIN_POLICY_COMPLIANCE`, the last step of provisioning: the app
 * is now device owner. It makes a home-screen app the home app and hides
 * the packages the QR code names ([RustDeviceAdmin.finishProvisioning]),
 * reports the admin extras bundle as a `provisioning_complete` event, then
 * lets the wizard finish — which launches home, so a home-screen app runs
 * with nobody opening it.
 */
class PolicyComplianceActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val extras: PersistableBundle? = if (Build.VERSION.SDK_INT >= 33) {
            intent.getParcelableExtra(DevicePolicyManager.EXTRA_PROVISIONING_ADMIN_EXTRAS_BUNDLE, PersistableBundle::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra(DevicePolicyManager.EXTRA_PROVISIONING_ADMIN_EXTRAS_BUNDLE)
        }
        RustDeviceAdmin.finishProvisioning(this, extras)
        RustDeviceAdmin.emitProvisioningComplete(this, extras)
        setResult(RESULT_OK)
        finish()
    }
}
