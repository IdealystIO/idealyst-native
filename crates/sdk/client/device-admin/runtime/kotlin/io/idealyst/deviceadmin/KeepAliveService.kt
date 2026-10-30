package io.idealyst.deviceadmin

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder

/**
 * A foreground service that keeps the device-owner app's process running
 * while it is in the background.
 *
 * Android 12+ freezes a cached background process outright (the cached-apps
 * freezer): the process stays alive but no thread runs, so timers, sockets
 * and the app's async work stop until it returns to the foreground. A
 * running foreground service keeps the process out of the cached state.
 *
 * Declared with `foregroundServiceType="systemExempted"`, the type Android
 * 14+ reserves for device- and profile-owner apps: no runtime limit and no
 * user-facing restriction. Started and stopped through
 * `device_admin::start_keep_alive` / `stop_keep_alive`.
 */
class KeepAliveService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val title = intent?.getStringExtra(EXTRA_TITLE) ?: applicationInfo.loadLabel(packageManager).toString()
        val text = intent?.getStringExtra(EXTRA_TEXT) ?: ""
        val notification = notification(this, title, text)
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(NOTIFICATION_ID, notification, FOREGROUND_SERVICE_TYPE_SYSTEM_EXEMPTED)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
        // Restarted by the system if it kills the process.
        return START_STICKY
    }

    companion object {
        const val EXTRA_TITLE = "io.idealyst.deviceadmin.KEEP_ALIVE_TITLE"
        const val EXTRA_TEXT = "io.idealyst.deviceadmin.KEEP_ALIVE_TEXT"
        private const val CHANNEL_ID = "io.idealyst.deviceadmin.keep_alive"
        private const val NOTIFICATION_ID = 0x1DEA
        /** `ServiceInfo.FOREGROUND_SERVICE_TYPE_SYSTEM_EXEMPTED` (API 34). */
        private const val FOREGROUND_SERVICE_TYPE_SYSTEM_EXEMPTED = 1 shl 10

        private fun notification(ctx: Context, title: String, text: String): Notification {
            if (Build.VERSION.SDK_INT >= 26) {
                val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
                // IMPORTANCE_LOW: in the shade, no sound or heads-up.
                nm.createNotificationChannel(
                    NotificationChannel(CHANNEL_ID, title, NotificationManager.IMPORTANCE_LOW)
                )
            }
            val builder = if (Build.VERSION.SDK_INT >= 26) {
                Notification.Builder(ctx, CHANNEL_ID)
            } else {
                @Suppress("DEPRECATION")
                Notification.Builder(ctx)
            }
            return builder
                .setContentTitle(title)
                .setContentText(text)
                // The app's icon; a system one when the app declares none
                // (startForeground rejects a notification without an icon).
                .setSmallIcon(ctx.applicationInfo.icon.takeIf { it != 0 } ?: android.R.drawable.ic_dialog_info)
                .setOngoing(true)
                .build()
        }
    }
}
