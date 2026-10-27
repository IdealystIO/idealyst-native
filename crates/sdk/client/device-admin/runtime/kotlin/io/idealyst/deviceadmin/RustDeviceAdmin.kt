package io.idealyst.deviceadmin

import android.app.Activity
import android.app.ActivityManager
import android.app.PendingIntent
import android.app.admin.DevicePolicyManager
import android.app.admin.SystemUpdatePolicy
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageInfo
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PersistableBundle
import java.io.File
import java.security.MessageDigest
import org.json.JSONArray
import org.json.JSONObject

/**
 * The device-admin SDK's JVM half. Rust reaches everything through [call]:
 * one JNI signature, a method name and JSON arguments, a JSON reply —
 * `{"ok": value}`, `{"pending": true}` for an operation that completes later
 * through [nativeResult], or `{"error": code, "message": text}` with `code`
 * one of `not_device_owner`, `unsupported`, `certificate_mismatch`,
 * `platform`.
 *
 * Events from the OS (provisioning, admin state, lock task, installs,
 * boot) go through [emit]. While Rust listens ([attachEvents]) they are
 * delivered at once through [nativeEvent]; otherwise they are appended to a
 * SharedPreferences queue, because the receivers and provisioning
 * activities run before — or without — the app's Rust side: a boot
 * broadcast or the provisioning hand-off starts the process with no
 * MainActivity, so the native library is not loaded and nothing could
 * receive them. The queue is drained, one event per looper message, when
 * Rust next attaches.
 *
 * Shipped from the `device-admin` SDK crate via
 * `[package.metadata.idealyst.android].runtime_kotlin`; the `native*`
 * symbols are the `#[no_mangle]` exports in its `android.rs`.
 */
object RustDeviceAdmin {
    private const val PREFS = "io.idealyst.deviceadmin"
    private const val QUEUE_KEY = "pending_events"
    private const val PROVISIONED_KEY = "provisioning_reported"
    private const val ACTION_PACKAGE_STATUS = "io.idealyst.deviceadmin.PACKAGE_STATUS"
    private const val EXTRA_TOKEN = "io.idealyst.deviceadmin.TOKEN"
    private const val EXTRA_OPERATION = "io.idealyst.deviceadmin.OPERATION"
    /** Oldest events are dropped past this, so a device that never runs the app cannot grow the queue without bound. */
    private const val QUEUE_LIMIT = 64

    private val main = Handler(Looper.getMainLooper())

    /** Touched only on the main thread. */
    private var nativeReady = false
    private var packageStatusReceiver: BroadcastReceiver? = null
    private var packageReplacedReceiver: BroadcastReceiver? = null

    private class Failure(val code: String, message: String) : Exception(message)

    private object Pending

    @JvmStatic
    fun call(context: Context, method: String, argsJson: String, token: Long): String {
        return try {
            val args = JSONObject(argsJson)
            when (val value = dispatch(context, method, args, token)) {
                Pending -> JSONObject().put("pending", true).toString()
                else -> JSONObject().put("ok", value ?: JSONObject.NULL).toString()
            }
        } catch (f: Failure) {
            error(f.code, f.message)
        } catch (e: SecurityException) {
            // A policy the device owner itself may not set on this build.
            error("platform", "SecurityException: ${e.message}")
        } catch (t: Throwable) {
            error("platform", "${t.javaClass.simpleName}: ${t.message}")
        }
    }

    private fun error(code: String, message: String?): String =
        JSONObject().put("error", code).put("message", message ?: "").toString()

    private fun dpm(ctx: Context) =
        ctx.getSystemService(Context.DEVICE_POLICY_SERVICE) as DevicePolicyManager

    fun admin(ctx: Context) = ComponentName(ctx, AdminReceiver::class.java)

    private fun requireOwner(ctx: Context) {
        if (!dpm(ctx).isDeviceOwnerApp(ctx.packageName)) {
            throw Failure("not_device_owner", "${ctx.packageName} is not the device owner")
        }
    }

    private fun requireApi(level: Int, what: String) {
        if (Build.VERSION.SDK_INT < level) {
            throw Failure("unsupported", "$what needs API $level; this device runs API ${Build.VERSION.SDK_INT}")
        }
    }

    private fun activity(ctx: Context): Activity =
        ctx as? Activity ?: throw Failure("platform", "lock task needs the app's Activity; the context is ${ctx.javaClass.name}")

    private fun strings(args: JSONObject, key: String): Array<String> {
        val arr = args.getJSONArray(key)
        return Array(arr.length()) { arr.getString(it) }
    }

    private fun dispatch(ctx: Context, method: String, args: JSONObject, token: Long): Any? {
        val dpm = dpm(ctx)
        when (method) {
            "is_device_owner" -> return dpm.isDeviceOwnerApp(ctx.packageName)
            "set_lock_task_packages" -> {
                requireOwner(ctx)
                dpm.setLockTaskPackages(admin(ctx), strings(args, "packages"))
            }
            "set_lock_task_features" -> {
                requireApi(28, "setLockTaskFeatures")
                requireOwner(ctx)
                dpm.setLockTaskFeatures(admin(ctx), args.getInt("flags"))
            }
            "start_lock_task" -> {
                val activity = activity(ctx)
                // Allow-listed: lock task starts silently. Otherwise Android
                // falls back to screen pinning, which asks the user.
                val permitted = Build.VERSION.SDK_INT >= 23 && dpm.isLockTaskPermitted(ctx.packageName)
                main.post {
                    try {
                        activity.startLockTask()
                        nativeResult(token, JSONObject().put("ok", permitted).toString())
                    } catch (t: Throwable) {
                        nativeResult(token, error("platform", "${t.javaClass.simpleName}: ${t.message}"))
                    }
                }
                return Pending
            }
            "stop_lock_task" -> {
                val activity = activity(ctx)
                main.post {
                    try {
                        activity.stopLockTask()
                        nativeResult(token, JSONObject().put("ok", true).toString())
                    } catch (t: Throwable) {
                        nativeResult(token, error("platform", "${t.javaClass.simpleName}: ${t.message}"))
                    }
                }
                return Pending
            }
            "lock_task_state" -> {
                val am = ctx.getSystemService(Context.ACTIVITY_SERVICE) as ActivityManager
                return if (Build.VERSION.SDK_INT >= 23) {
                    when (am.lockTaskModeState) {
                        ActivityManager.LOCK_TASK_MODE_LOCKED -> "locked"
                        ActivityManager.LOCK_TASK_MODE_PINNED -> "pinned"
                        else -> "none"
                    }
                } else {
                    @Suppress("DEPRECATION")
                    if (am.isInLockTaskMode) "locked" else "none"
                }
            }
            "set_keyguard_disabled" -> {
                requireApi(23, "setKeyguardDisabled")
                requireOwner(ctx)
                if (!dpm.setKeyguardDisabled(admin(ctx), args.getBoolean("disabled"))) {
                    throw Failure("platform", "the keyguard could not be changed (is a screen lock credential set?)")
                }
            }
            "set_status_bar_disabled" -> {
                requireApi(23, "setStatusBarDisabled")
                requireOwner(ctx)
                if (!dpm.setStatusBarDisabled(admin(ctx), args.getBoolean("disabled"))) {
                    throw Failure("platform", "the status bar could not be changed")
                }
            }
            "set_home_activity" -> {
                requireOwner(ctx)
                val pkg = args.getString("package")
                val activityName = args.getString("activity").let { if (it.startsWith(".")) pkg + it else it }
                val filter = IntentFilter(Intent.ACTION_MAIN).apply {
                    addCategory(Intent.CATEGORY_HOME)
                    addCategory(Intent.CATEGORY_DEFAULT)
                }
                dpm.addPersistentPreferredActivity(admin(ctx), filter, ComponentName(pkg, activityName))
            }
            "clear_home_activity" -> {
                requireOwner(ctx)
                dpm.clearPackagePersistentPreferredActivities(admin(ctx), args.getString("package"))
            }
            "add_user_restriction" -> {
                requireOwner(ctx)
                dpm.addUserRestriction(admin(ctx), args.getString("restriction"))
            }
            "clear_user_restriction" -> {
                requireOwner(ctx)
                dpm.clearUserRestriction(admin(ctx), args.getString("restriction"))
            }
            "set_global_setting" -> {
                requireOwner(ctx)
                dpm.setGlobalSetting(admin(ctx), args.getString("key"), args.getString("value"))
            }
            "install_apk" -> {
                requireOwner(ctx)
                install(ctx, File(args.getString("path")), args.getString("expected_cert_sha256"), token)
                return Pending
            }
            "uninstall" -> {
                requireOwner(ctx)
                val pkg = args.getString("package")
                ensurePackageStatusReceiver(ctx)
                ctx.packageManager.packageInstaller.uninstall(pkg, statusIntent(ctx, token, "uninstall").intentSender)
                return Pending
            }
            "installed_version" -> {
                val info = try {
                    ctx.packageManager.getPackageInfo(args.getString("package"), 0)
                } catch (e: PackageManager.NameNotFoundException) {
                    return null
                }
                return JSONObject().put("name", info.versionName ?: JSONObject.NULL).put("code", versionCode(info))
            }
            "launch" -> {
                val pkg = args.getString("package")
                val intent = ctx.packageManager.getLaunchIntentForPackage(pkg)
                    ?: throw Failure("platform", "$pkg has no launchable activity (is it installed?)")
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                ctx.startActivity(intent)
            }
            "reboot" -> {
                requireApi(24, "reboot")
                requireOwner(ctx)
                dpm.reboot(admin(ctx))
            }
            "set_system_update_policy" -> {
                requireApi(23, "setSystemUpdatePolicy")
                requireOwner(ctx)
                val policy = when (args.getString("kind")) {
                    "automatic" -> SystemUpdatePolicy.createAutomaticInstallPolicy()
                    "windowed" -> SystemUpdatePolicy.createWindowedInstallPolicy(args.getInt("start_minutes"), args.getInt("end_minutes"))
                    "postpone" -> SystemUpdatePolicy.createPostponeInstallPolicy()
                    else -> null
                }
                dpm.setSystemUpdatePolicy(admin(ctx), policy)
            }
            "wipe" -> {
                requireOwner(ctx)
                val reason = args.optString("reason", "")
                when {
                    // API 34 refuses wipeData from a device owner on the
                    // system user; wipeDevice is its replacement.
                    Build.VERSION.SDK_INT >= 34 -> dpm.wipeDevice(0)
                    Build.VERSION.SDK_INT >= 28 && reason.isNotEmpty() -> dpm.wipeData(0, reason)
                    else -> dpm.wipeData(0)
                }
            }
            "relinquish_device_owner" -> {
                requireOwner(ctx)
                @Suppress("DEPRECATION")
                dpm.clearDeviceOwnerApp(ctx.packageName)
            }
            "attach_events" -> {
                main.post { attachEvents(ctx.applicationContext) }
            }
            "detach_events" -> {
                main.post { detachEvents(ctx.applicationContext) }
            }
            else -> throw Failure("unsupported", "unknown device-admin call $method")
        }
        return null
    }

    // ── Installs ────────────────────────────────────────────────────────

    private fun versionCode(info: PackageInfo): Long =
        if (Build.VERSION.SDK_INT >= 28) info.longVersionCode else @Suppress("DEPRECATION") info.versionCode.toLong()

    private fun certSha256(ctx: Context, apk: File): String? {
        val pm = ctx.packageManager
        val certs = if (Build.VERSION.SDK_INT >= 28) {
            val info = pm.getPackageArchiveInfo(apk.path, PackageManager.GET_SIGNING_CERTIFICATES) ?: return null
            info.signingInfo?.apkContentsSigners
        } else {
            @Suppress("DEPRECATION")
            pm.getPackageArchiveInfo(apk.path, PackageManager.GET_SIGNATURES)?.signatures
        } ?: return null
        if (certs.size != 1) return null
        val digest = MessageDigest.getInstance("SHA-256").digest(certs[0].toByteArray())
        return digest.joinToString("") { "%02x".format(it) }
    }

    private fun install(ctx: Context, apk: File, expectedHex: String, token: Long) {
        if (!apk.isFile) throw Failure("platform", "no APK at ${apk.path}")
        // Checked before anything is written to a session, so a wrong build
        // never reaches the installer. Android would refuse a different
        // signer for an update anyway; this also guards a first install.
        val actual = certSha256(ctx, apk)
            ?: throw Failure("platform", "${apk.path} is not a readable APK with exactly one signer")
        val expected = expectedHex.replace(":", "").lowercase()
        if (actual != expected) {
            throw Failure("certificate_mismatch", "expected=$expected actual=$actual")
        }
        ensurePackageStatusReceiver(ctx)
        val sender = statusIntent(ctx, token, "install").intentSender
        // Copying a large APK into the session is slow; keep it off the
        // caller's (often the UI) thread.
        Thread {
            try {
                val installer = ctx.packageManager.packageInstaller
                val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
                if (Build.VERSION.SDK_INT >= 31) {
                    params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
                }
                val sessionId = installer.createSession(params)
                installer.openSession(sessionId).use { session ->
                    apk.inputStream().use { input ->
                        session.openWrite("base.apk", 0, apk.length()).use { out ->
                            input.copyTo(out)
                            session.fsync(out)
                        }
                    }
                    session.commit(sender)
                }
            } catch (t: Throwable) {
                main.post {
                    nativeResult(token, error("platform", "${t.javaClass.simpleName}: ${t.message}"))
                }
            }
        }.start()
    }

    private fun statusIntent(ctx: Context, token: Long, operation: String): PendingIntent {
        val intent = Intent(ACTION_PACKAGE_STATUS)
            .setPackage(ctx.packageName)
            .putExtra(EXTRA_TOKEN, token)
            .putExtra(EXTRA_OPERATION, operation)
        // MUTABLE: the installer fills in the status extras.
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            (if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0)
        return PendingIntent.getBroadcast(ctx, token.toInt(), intent, flags)
    }

    private fun ensurePackageStatusReceiver(ctx: Context) {
        val app = ctx.applicationContext
        main.post {
            if (packageStatusReceiver != null) return@post
            val receiver = object : BroadcastReceiver() {
                override fun onReceive(context: Context, intent: Intent) = onPackageStatus(context, intent)
            }
            register(app, receiver, IntentFilter(ACTION_PACKAGE_STATUS), exported = false)
            packageStatusReceiver = receiver
        }
    }

    private fun register(ctx: Context, receiver: BroadcastReceiver, filter: IntentFilter, exported: Boolean) {
        if (Build.VERSION.SDK_INT >= 33) {
            ctx.registerReceiver(receiver, filter, if (exported) Context.RECEIVER_EXPORTED else Context.RECEIVER_NOT_EXPORTED)
        } else {
            ctx.registerReceiver(receiver, filter)
        }
    }

    private fun onPackageStatus(ctx: Context, intent: Intent) {
        val token = intent.getLongExtra(EXTRA_TOKEN, 0L)
        val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
        // PENDING_USER_ACTION is only reachable when the app is not (or no
        // longer) the device owner. It is reported as a failure; the
        // confirmation dialog it carries is never started.
        val message = if (status == PackageInstaller.STATUS_PENDING_USER_ACTION) {
            "the installer asked for user confirmation; silent installs need the device owner"
        } else {
            intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
        }
        val pkg = intent.getStringExtra(PackageInstaller.EXTRA_PACKAGE_NAME)
        val result = JSONObject()
            .put("type", "install_result")
            .put("operation", intent.getStringExtra(EXTRA_OPERATION) ?: "install")
            .put("package", pkg ?: JSONObject.NULL)
            .put("status", status)
            .put("message", message ?: JSONObject.NULL)
        if (status == PackageInstaller.STATUS_SUCCESS && pkg != null) {
            try {
                result.put("version_code", versionCode(ctx.packageManager.getPackageInfo(pkg, 0)))
            } catch (e: PackageManager.NameNotFoundException) {
                // Uninstalled: no version to report.
            }
        }
        if (token != 0L) {
            nativeResult(token, JSONObject().put("ok", result).toString())
        }
        emit(ctx, result)
    }

    // ── Events ──────────────────────────────────────────────────────────

    /** Main thread. Deliver now when Rust listens; queue otherwise. */
    fun emit(ctx: Context, event: JSONObject) {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            main.post { emit(ctx, event) }
            return
        }
        val prefs = ctx.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val queue = JSONArray(prefs.getString(QUEUE_KEY, "[]"))
        // Deliver at once only behind an empty queue, so events stay in the
        // order they happened while a drain is in progress.
        if (nativeReady && queue.length() == 0) {
            nativeEvent(event.toString())
            return
        }
        queue.put(event)
        while (queue.length() > QUEUE_LIMIT) queue.remove(0)
        // commit, not apply: a receiver's process can be killed as soon as
        // onReceive returns, before an async write lands.
        prefs.edit().putString(QUEUE_KEY, queue.toString()).commit()
    }

    /** Provisioning can be reported by both the compliance activity and the admin receiver's broadcast; deliver it once. */
    fun emitProvisioningComplete(ctx: Context, extras: PersistableBundle?) {
        val prefs = ctx.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        if (prefs.getBoolean(PROVISIONED_KEY, false)) return
        prefs.edit().putBoolean(PROVISIONED_KEY, true).commit()
        emit(ctx, JSONObject().put("type", "provisioning_complete").put("extras", bundleToJson(extras)))
    }

    private fun attachEvents(app: Context) {
        nativeReady = true
        if (packageReplacedReceiver == null) {
            // ACTION_PACKAGE_REPLACED for other packages is not delivered to
            // manifest receivers (API 26 implicit-broadcast limits), so it is
            // only seen while the app runs.
            val receiver = object : BroadcastReceiver() {
                override fun onReceive(context: Context, intent: Intent) {
                    val pkg = intent.data?.schemeSpecificPart ?: return
                    emit(context, JSONObject().put("type", "package_replaced").put("package", pkg))
                }
            }
            val filter = IntentFilter(Intent.ACTION_PACKAGE_REPLACED).apply { addDataScheme("package") }
            register(app, receiver, filter, exported = true)
            packageReplacedReceiver = receiver
        }
        drainQueue(app)
    }

    /**
     * Deliver the oldest queued event, then post the next delivery as its
     * own looper message. One event per message matters: a listener's
     * signal writes stage and commit in a flush the Rust side posts to the
     * looper during delivery, so the next event must be posted after it —
     * otherwise a listener that appends to a signal reads the uncommitted
     * value and the earlier event is lost.
     */
    private fun drainQueue(app: Context) {
        if (!nativeReady) return
        val prefs = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val queue = JSONArray(prefs.getString(QUEUE_KEY, "[]"))
        if (queue.length() == 0) return
        val first = queue.getJSONObject(0)
        queue.remove(0)
        prefs.edit().putString(QUEUE_KEY, queue.toString()).commit()
        nativeEvent(first.toString())
        if (queue.length() > 0) main.post { drainQueue(app) }
    }

    private fun detachEvents(app: Context) {
        nativeReady = false
        packageReplacedReceiver?.let { app.unregisterReceiver(it) }
        packageReplacedReceiver = null
    }

    fun bundleToJson(bundle: PersistableBundle?): JSONObject {
        val out = JSONObject()
        if (bundle == null) return out
        for (key in bundle.keySet()) {
            @Suppress("DEPRECATION")
            out.put(key, when (val v = bundle.get(key)) {
                null -> JSONObject.NULL
                is PersistableBundle -> bundleToJson(v)
                is Array<*> -> JSONArray(v.toList())
                is IntArray -> JSONArray(v.toList())
                is LongArray -> JSONArray(v.toList())
                is DoubleArray -> JSONArray(v.toList())
                is BooleanArray -> JSONArray(v.toList())
                else -> v
            })
        }
        return out
    }

    @JvmStatic
    private external fun nativeResult(token: Long, json: String)

    @JvmStatic
    private external fun nativeEvent(json: String)
}
