package io.idealyst.screenrecorder

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder

/**
 * The mandatory **mediaProjection foreground service** for screen capture.
 *
 * ## Why it's required (and why ordering matters)
 *
 * On Android 14 (API 34 — the emulator target) `MediaProjectionManager
 * .getMediaProjection(resultCode, data)` throws `SecurityException` unless a
 * foreground service whose `foregroundServiceType` includes `mediaProjection`
 * has **already** been `startForeground(...)`-ed. So the capture flow is
 * strictly ordered:
 *
 *   consent OK → startForegroundService(this) → onStartCommand → startForeground
 *   → (the service signals back) → getMediaProjection → createVirtualDisplay
 *
 * `RustScreenCaptureHelper` drives that ordering: it starts this service and
 * only calls `getMediaProjection` once the service has invoked
 * [onForegrounded]. This service itself holds no capture state — it exists
 * purely to satisfy the FGS requirement; the helper owns the projection +
 * virtual display + image reader.
 *
 * Declared in the app manifest by `run-android` (the `screen_capture`
 * capability's `android_service`):
 * `<service android:name="io.idealyst.screenrecorder.MediaProjectionService"
 *           android:foregroundServiceType="mediaProjection"
 *           android:exported="false"/>`.
 */
class MediaProjectionService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        ensureChannel(this)
        val notification = buildNotification(this)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            // The type MUST match the manifest's foregroundServiceType, or
            // Android rejects the startForeground call on API 29+.
            startForeground(
                NOTIFICATION_ID,
                notification,
                android.content.pm.ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
        // The service is now foregrounded — unblock the helper's
        // getMediaProjection call. STICKY isn't wanted: capture is torn down
        // explicitly via stop(), and a restarted service with no helper state
        // would do nothing useful.
        onForegrounded()
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        super.onDestroy()
        // Drop the foreground state + notification. The helper stops the
        // projection itself; this just clears the FGS.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            stopForeground(true)
        }
    }

    companion object {
        private const val CHANNEL_ID = "idealyst_screen_capture"
        private const val NOTIFICATION_ID = 0x5C12

        /**
         * Set by [RustScreenCaptureHelper] before it starts this service, so
         * the service can signal "I'm foregrounded; proceed to
         * getMediaProjection" without a binder. Null after capture stops.
         */
        @Volatile
        @JvmStatic
        var onForegrounded: () -> Unit = {}

        private fun ensureChannel(context: Context) {
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
            val mgr =
                context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            if (mgr.getNotificationChannel(CHANNEL_ID) == null) {
                val channel = NotificationChannel(
                    CHANNEL_ID,
                    "Screen capture",
                    NotificationManager.IMPORTANCE_LOW,
                )
                channel.description = "Active while the app is sharing the screen."
                mgr.createNotificationChannel(channel)
            }
        }

        private fun buildNotification(context: Context): Notification {
            val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                Notification.Builder(context, CHANNEL_ID)
            } else {
                @Suppress("DEPRECATION")
                Notification.Builder(context)
            }
            return builder
                .setContentTitle("Sharing screen")
                .setContentText("This app is capturing the screen.")
                // A built-in platform icon avoids shipping a drawable resource
                // through the run-android res pipeline.
                .setSmallIcon(android.R.drawable.ic_menu_camera)
                .setOngoing(true)
                .build()
        }
    }
}
