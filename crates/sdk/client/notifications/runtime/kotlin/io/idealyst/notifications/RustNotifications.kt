package io.idealyst.notifications

import android.app.Activity
import android.app.AlarmManager
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.RemoteInput
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject

/**
 * Android half of the `notifications` SDK. Rust (`src/android.rs`) calls
 * [builder] to get a `Notification.Builder` with the shared fields and every
 * tap / action / dismiss intent wired, runs the app's builder hook on it,
 * then [show] posts it now or hands it to `AlarmManager` for later.
 *
 * Responses come back through two components declared in this SDK's
 * manifest fragment, so the generated `MainActivity` needs no edits:
 *
 *  - [NotificationOpenActivity] — a translucent activity that receives a
 *    tap on the body and every action that opens the app. Android 12+
 *    blocks starting an activity from a receiver or service a notification
 *    launched ("notification trampolines"), so whatever opens the app must
 *    itself be an activity. It reports the response, starts the app's
 *    launch intent (bringing a running app's task to the front) and
 *    finishes.
 *  - [NotificationReceiver] — actions that do not open the app, the
 *    dismiss (delete) intent, and the alarm of a scheduled notification.
 *
 * A response is handed to Rust through [nativeResponse] when the Rust side
 * has attached in this process; before that (the tap that cold-starts the
 * app, an action pressed while the app is not running) it is appended to a
 * queue in `SharedPreferences`, which Rust drains in [attach] at boot. The
 * queue survives the process: a background action in a dead app is
 * delivered on its next launch.
 *
 * Every component runs on the main thread, so [dispatch] and the Rust
 * callback do too.
 *
 * VERIFICATION: compile-checked against the Rust JNI signatures; the
 * notification, alarm and intent paths resolve only on a device.
 */
object RustNotifications {
    private const val TAG = "RustNotifications"
    private const val PREFS = "io.idealyst.notifications"
    private const val PENDING = "pending"
    private const val SCHEDULED = "scheduled"

    const val ACTION_RESPOND = "io.idealyst.notifications.RESPOND"
    const val ACTION_SHOW = "io.idealyst.notifications.SHOW"
    const val EXTRA_ID = "io.idealyst.notifications.id"
    const val EXTRA_TAG = "io.idealyst.notifications.tag"
    const val EXTRA_ACTION = "io.idealyst.notifications.action"
    const val EXTRA_DATA = "io.idealyst.notifications.data"
    const val EXTRA_NOTIFICATION = "io.idealyst.notifications.notification"
    const val REPLY_KEY = "io.idealyst.notifications.reply"

    /** The response a tap on the body reports. */
    const val OPEN = "open"
    /** The response the delete intent reports. */
    const val DISMISS = "dismiss"
    /** Prefix of an action id in [EXTRA_ACTION]. */
    const val ACTION_PREFIX = "action:"

    // android.app.NotificationManager importance levels.
    private const val IMPORTANCE_LOW = 2
    private const val IMPORTANCE_DEFAULT = 3
    private const val IMPORTANCE_HIGH = 4

    /** Android shows at most three action buttons. */
    private const val MAX_ACTIONS = 3

    @Volatile
    private var attached = false

    /** Hands one response to Rust (`src/android.rs`). Main thread. */
    @JvmStatic
    external fun nativeResponse(id: String, action: String, text: String?, dataJson: String)

    /**
     * Rust has booted in this process: deliver directly from now on, and
     * return (and clear) what queued before, as a JSON array of
     * `{id, action, text, data}`.
     */
    @JvmStatic
    fun attach(context: Context): String {
        synchronized(this) {
            attached = true
            val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val pending = prefs.getString(PENDING, "[]") ?: "[]"
            prefs.edit().remove(PENDING).apply()
            return pending
        }
    }

    /** Report a response: to Rust when attached, else to the queue. */
    @JvmStatic
    fun dispatch(context: Context, id: String, action: String, text: String?, dataJson: String) {
        synchronized(this) {
            if (attached) {
                try {
                    nativeResponse(id, action, text, dataJson)
                    return
                } catch (e: UnsatisfiedLinkError) {
                    Log.w(TAG, "native side not loaded; queueing response", e)
                }
            }
            val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val queue = JSONArray(prefs.getString(PENDING, "[]") ?: "[]")
            queue.put(
                JSONObject()
                    .put("id", id)
                    .put("action", action)
                    .put("text", text ?: JSONObject.NULL)
                    .put("data", JSONObject(dataJson))
            )
            prefs.edit().putString(PENDING, queue.toString()).apply()
        }
    }

    /**
     * A builder carrying the shared fields, on the channel for [priority]
     * (0 low, 1 default, 2 high, 3 urgent) — or the silent channel when
     * [silent] — named [channelName], with the
     * tap, action and dismiss intents for notification [id] posted under
     * the int [tag]. [actionsJson] is an array of `{id, title, input:
     * {button, placeholder} | null, opens_app}`.
     */
    @JvmStatic
    fun builder(
        context: Context,
        id: String,
        tag: Int,
        title: String,
        body: String,
        subtitle: String?,
        group: String?,
        priority: Int,
        channelName: String,
        silent: Boolean,
        image: String?,
        dataJson: String,
        actionsJson: String,
    ): Notification.Builder {
        val channel = ensureChannel(context, priority, silent, channelName)
        val b = Notification.Builder(context, channel)
            .setContentTitle(title)
            .setContentText(body)
            .setSmallIcon(context.applicationInfo.icon)
            .setAutoCancel(true)
            .setContentIntent(openIntent(context, id, tag, OPEN, dataJson, mutable = false))
            .setDeleteIntent(receiverIntent(context, id, tag, DISMISS, dataJson, mutable = false))
        if (subtitle != null) b.setSubText(subtitle)
        if (group != null) b.setGroup(group)
        if (image != null) {
            val bitmap = BitmapFactory.decodeFile(image)
            if (bitmap != null) {
                b.setLargeIcon(bitmap)
                b.setStyle(Notification.BigPictureStyle().bigPicture(bitmap))
            } else {
                Log.w(TAG, "could not decode notification image $image")
            }
        }
        val actions = JSONArray(actionsJson)
        for (i in 0 until minOf(actions.length(), MAX_ACTIONS)) {
            b.addAction(action(context, id, tag, dataJson, actions.getJSONObject(i)))
        }
        return b
    }

    private fun action(context: Context, id: String, tag: Int, dataJson: String, a: JSONObject): Notification.Action {
        val actionId = ACTION_PREFIX + a.getString("id")
        val input = a.optJSONObject("input")
        val opensApp = a.optBoolean("opens_app", true)
        // A RemoteInput writes the reply into the intent, so its intent
        // must be mutable (API 31 requires saying so).
        val mutable = input != null
        val intent = if (opensApp) {
            openIntent(context, id, tag, actionId, dataJson, mutable)
        } else {
            receiverIntent(context, id, tag, actionId, dataJson, mutable)
        }
        val label = if (input != null) input.getString("button") else a.getString("title")
        val builder = Notification.Action.Builder(null, label, intent)
        if (input != null) {
            builder.addRemoteInput(RemoteInput.Builder(REPLY_KEY).setLabel(input.getString("placeholder")).build())
        }
        return builder.build()
    }

    /** Post [builder] now, or after [delayMs] through `AlarmManager`. */
    @JvmStatic
    fun show(context: Context, tag: Int, builder: Notification.Builder, delayMs: Long) {
        val notification = builder.build()
        val manager = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (delayMs <= 0) {
            manager.notify(tag, notification)
            return
        }
        // The built notification rides in the alarm's intent (it is
        // Parcelable), so the receiver posts exactly what was built here,
        // hooks included. Alarms do not survive a reboot.
        val alarm = Intent(context, NotificationReceiver::class.java)
            .setAction(ACTION_SHOW)
            .setData(Uri.parse("idealyst-notification://$tag/show"))
            .putExtra(EXTRA_TAG, tag)
            .putExtra(EXTRA_NOTIFICATION, notification)
        val pending = PendingIntent.getBroadcast(context, 0, alarm, flags(mutable = false))
        val alarms = context.getSystemService(Context.ALARM_SERVICE) as AlarmManager
        val at = SystemClock.elapsedRealtime() + delayMs
        // An exact alarm needs the user-granted SCHEDULE_EXACT_ALARM on API
        // 31+; without it the alarm is inexact (the OS may batch it).
        if (Build.VERSION.SDK_INT < 31 || alarms.canScheduleExactAlarms()) {
            alarms.setExactAndAllowWhileIdle(AlarmManager.ELAPSED_REALTIME_WAKEUP, at, pending)
        } else {
            alarms.setAndAllowWhileIdle(AlarmManager.ELAPSED_REALTIME_WAKEUP, at, pending)
        }
        rememberScheduled(context, tag, true)
    }

    /** Take back notification [tag]: shown, and pending in an alarm. */
    @JvmStatic
    fun cancel(context: Context, tag: Int) {
        val manager = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        manager.cancel(tag)
        val alarm = Intent(context, NotificationReceiver::class.java)
            .setAction(ACTION_SHOW)
            .setData(Uri.parse("idealyst-notification://$tag/show"))
        val pending = PendingIntent.getBroadcast(context, 0, alarm, flags(mutable = false) or PendingIntent.FLAG_NO_CREATE)
        if (pending != null) {
            (context.getSystemService(Context.ALARM_SERVICE) as AlarmManager).cancel(pending)
            pending.cancel()
        }
        rememberScheduled(context, tag, false)
    }

    /** Take back every notification this app posted or scheduled. */
    @JvmStatic
    fun cancelAll(context: Context) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        for (tag in prefs.getStringSet(SCHEDULED, emptySet()).orEmpty().toList()) {
            tag.toIntOrNull()?.let { cancel(context, it) }
        }
        (context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager).cancelAll()
    }

    /** Track scheduled tags, so [cancelAll] can find their alarms. */
    @JvmStatic
    fun rememberScheduled(context: Context, tag: Int, scheduled: Boolean) {
        synchronized(this) {
            val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val tags = prefs.getStringSet(SCHEDULED, emptySet()).orEmpty().toMutableSet()
            if (scheduled) tags.add(tag.toString()) else tags.remove(tag.toString())
            prefs.edit().putStringSet(SCHEDULED, tags).apply()
        }
    }

    /**
     * The channel for [priority], or the silent channel, created or renamed
     * to [name]. Since Android 8 a notification's sound belongs to its
     * channel, so silence is a channel of its own: normal importance (in
     * the shade and status bar, no heads-up) with no sound or vibration.
     * Idempotent: `createNotificationChannel` keeps an existing channel's
     * settings and updates its name.
     */
    private fun ensureChannel(context: Context, priority: Int, silent: Boolean, name: String): String {
        val manager = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (silent) {
            val channel = NotificationChannel("idealyst_silent", name, IMPORTANCE_DEFAULT)
            channel.setSound(null, null)
            channel.enableVibration(false)
            manager.createNotificationChannel(channel)
            return channel.id
        }
        val (id, importance) = when (priority) {
            0 -> "idealyst_low" to IMPORTANCE_LOW
            2 -> "idealyst_high" to IMPORTANCE_HIGH
            3 -> "idealyst_urgent" to IMPORTANCE_HIGH
            else -> "idealyst_default" to IMPORTANCE_DEFAULT
        }
        manager.createNotificationChannel(NotificationChannel(id, name, importance))
        return id
    }

    /** The intent extras every response intent carries. */
    private fun fill(intent: Intent, id: String, tag: Int, action: String, dataJson: String): Intent =
        intent
            // Unique per notification and action: PendingIntents are
            // matched on (action, data, component), so without a distinct
            // data URI one notification's extras would overwrite another's.
            .setData(Uri.parse("idealyst-notification://$tag/" + Uri.encode(action)))
            .putExtra(EXTRA_ID, id)
            .putExtra(EXTRA_TAG, tag)
            .putExtra(EXTRA_ACTION, action)
            .putExtra(EXTRA_DATA, dataJson)

    private fun openIntent(context: Context, id: String, tag: Int, action: String, dataJson: String, mutable: Boolean): PendingIntent {
        val intent = fill(Intent(context, NotificationOpenActivity::class.java), id, tag, action, dataJson)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return PendingIntent.getActivity(context, 0, intent, flags(mutable))
    }

    private fun receiverIntent(context: Context, id: String, tag: Int, action: String, dataJson: String, mutable: Boolean): PendingIntent {
        val intent = fill(Intent(context, NotificationReceiver::class.java).setAction(ACTION_RESPOND), id, tag, action, dataJson)
        return PendingIntent.getBroadcast(context, 0, intent, flags(mutable))
    }

    private fun flags(mutable: Boolean): Int {
        val mutability = if (mutable && Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else PendingIntent.FLAG_IMMUTABLE
        return PendingIntent.FLAG_UPDATE_CURRENT or mutability
    }

    /**
     * Report the response an intent built by [fill] carries, and take the
     * notification down (a pressed action leaves it up otherwise; a
     * dismissed one is already gone).
     */
    @JvmStatic
    fun handle(context: Context, intent: Intent) {
        val id = intent.getStringExtra(EXTRA_ID) ?: return
        val action = intent.getStringExtra(EXTRA_ACTION) ?: OPEN
        val data = intent.getStringExtra(EXTRA_DATA) ?: "{}"
        val text = RemoteInput.getResultsFromIntent(intent)?.getCharSequence(REPLY_KEY)?.toString()
        if (action != DISMISS) {
            val manager = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            manager.cancel(intent.getIntExtra(EXTRA_TAG, 0))
        }
        dispatch(context, id, action, text, data)
    }
}

/** Receives taps and app-opening actions; see [RustNotifications]. */
class NotificationOpenActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        RustNotifications.handle(this, intent)
        val launch = packageManager.getLaunchIntentForPackage(packageName)
        if (launch != null) {
            launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_RESET_TASK_IF_NEEDED)
            startActivity(launch)
        }
        finish()
        @Suppress("DEPRECATION")
        overridePendingTransition(0, 0)
    }
}

/** Receives background actions, dismissals and alarms; see [RustNotifications]. */
class NotificationReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            RustNotifications.ACTION_SHOW -> {
                val tag = intent.getIntExtra(RustNotifications.EXTRA_TAG, 0)
                val notification = if (Build.VERSION.SDK_INT >= 33) {
                    intent.getParcelableExtra(RustNotifications.EXTRA_NOTIFICATION, Notification::class.java)
                } else {
                    @Suppress("DEPRECATION")
                    intent.getParcelableExtra(RustNotifications.EXTRA_NOTIFICATION)
                }
                if (notification != null) {
                    val manager = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
                    manager.notify(tag, notification)
                }
                RustNotifications.rememberScheduled(context, tag, false)
            }
            RustNotifications.ACTION_RESPOND -> RustNotifications.handle(context, intent)
        }
    }
}
