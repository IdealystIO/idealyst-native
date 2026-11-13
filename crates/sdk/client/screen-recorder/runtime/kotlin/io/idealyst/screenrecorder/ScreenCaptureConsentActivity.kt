package io.idealyst.screenrecorder

import android.app.Activity
import android.content.Intent
import android.os.Bundle

/**
 * Asks Android for the screen and hands the answer to
 * [RustScreenCaptureHelper].
 *
 * MediaProjection consent is an activity result, so something has to be an
 * activity. This one is see-through, in a task of its own and gone as soon as
 * it has the answer, so whatever is on screen stays there: an app capturing
 * from the background (a device-owner kiosk agent) does not bring its own
 * task forward, and with consent pre-granted (`PROJECT_MEDIA`) the system's
 * prompt closes before it draws.
 */
class ScreenCaptureConsentActivity : Activity() {
    private var token = 0L

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        token = intent.getLongExtra(EXTRA_TOKEN, 0L)
        if (savedInstanceState != null) return
        // Opened too late (the start gave up) or for a capture since stopped.
        if (!RustScreenCaptureHelper.consentOpened(token)) {
            close()
            return
        }
        try {
            @Suppress("DEPRECATION")
            startActivityForResult(RustScreenCaptureHelper.consentIntent(this), REQUEST)
        } catch (_: Throwable) {
            RustScreenCaptureHelper.onConsentResult(applicationContext, token, RESULT_CANCELED, null)
            close()
        }
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != REQUEST) return
        RustScreenCaptureHelper.onConsentResult(applicationContext, token, resultCode, data)
        close()
    }

    private fun close() {
        finish()
        @Suppress("DEPRECATION")
        overridePendingTransition(0, 0)
    }

    companion object {
        const val EXTRA_TOKEN = "io.idealyst.screenrecorder.token"
        private const val REQUEST = 0x5C12
    }
}
