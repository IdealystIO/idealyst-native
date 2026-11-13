package io.idealyst.accessibilityservice

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Path
import android.graphics.Point
import android.graphics.Rect
import android.hardware.display.DisplayManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.view.Display
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.json.JSONArray
import org.json.JSONObject

/**
 * The app's accessibility service. The system binds it once someone
 * switches it on in Settings > Accessibility, and it then does what Rust
 * asks through [RustAccessibility]. The only events it reads are window
 * changes, passed to Rust while it listens for them.
 */
class RustAccessibilityService : AccessibilityService() {
    override fun onServiceConnected() {
        super.onServiceConnected()
        RustAccessibility.connected(this)
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (event != null) RustAccessibility.event(event)
    }

    override fun onInterrupt() {}

    override fun onUnbind(intent: Intent?): Boolean {
        RustAccessibility.disconnected(this)
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        RustAccessibility.disconnected(this)
        super.onDestroy()
    }
}

/**
 * The accessibility-service SDK's JVM half. Rust reaches everything through
 * [call]: one JNI signature, a method name and JSON arguments, a JSON reply —
 * `{"ok": value}`, `{"pending": true}` for a screenshot or gesture whose
 * result arrives later through [nativeResult] / [nativeScreenshot], or
 * `{"error": code, "message": text}` with `code` one of `not_enabled`,
 * `too_soon`, `secure_window`, `gesture_cancelled`, `refused`,
 * `no_focused_input`, `stale_node`, `timeout`, `invalid_argument`,
 * `unsupported`, `platform`.
 *
 * Shipped from the `accessibility-service` SDK crate via
 * `[package.metadata.idealyst.android].runtime_kotlin`; the `native*`
 * symbols are the `#[no_mangle]` exports in its `android.rs`.
 */
object RustAccessibility {
    private val main = Handler(Looper.getMainLooper())

    /**
     * Screenshot conversion (hardware buffer → software bitmap → RGBA or
     * JPEG) runs here, never on the main thread: a 2560 × 1600 JPEG takes
     * tens of milliseconds.
     */
    private val worker = Executors.newSingleThreadScheduledExecutor()

    /**
     * Element searches, clicks and the window list. Every step of a tree walk
     * is a call into the app that owns the window, so it stays off the main
     * thread, and off [worker] so a long search does not hold a screenshot.
     */
    private val finder = Executors.newSingleThreadScheduledExecutor()

    /**
     * How long a screenshot waits for the secure-window probe once the
     * display capture is in. The probe is answered by the front app's own UI
     * thread, so a busy or hung app (the moment remote view is needed most)
     * would otherwise hold the screenshot for seconds; past this the frame
     * goes out unprobed.
     */
    private const val PROBE_GRACE_MS = 500L

    /** The bound service, or null. Written on the main thread, read from any. */
    @Volatile
    private var service: RustAccessibilityService? = null

    /** Rust has an enabled-change listener, so [nativeEnabled] may be called. */
    @Volatile
    private var listening = false

    /** Rust has a window listener, so [nativeWindowEvent] may be called. */
    @Volatile
    private var listeningWindows = false

    /**
     * Elements [find] returned, by the handle Rust holds, for [click]: the
     * most recent [nodeHandles], oldest dropped first. A node keeps its window and view id, so [click]
     * re-reads it with `refresh()` rather than trusting what was found.
     */
    private val nodes = object : LinkedHashMap<Long, AccessibilityNodeInfo>() {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Long, AccessibilityNodeInfo>?) =
            size > nodeHandles
    }
    /** Set from each `find`'s `node_handles`, so Rust's constant is the only one. */
    private var nodeHandles = 0
    private var nextNode = 1L

    private class Failure(val code: String, message: String) : Exception(message)

    private object Pending

    @JvmStatic external fun nativeResult(token: Long, json: String)

    @JvmStatic external fun nativeScreenshot(token: Long, width: Int, height: Int, rotation: Int, data: ByteArray)

    @JvmStatic external fun nativeEnabled(enabled: Boolean)

    @JvmStatic external fun nativeWindowEvent(json: String)

    /** An accessibility event, on the main thread: window changes go to Rust while it listens. */
    fun event(e: AccessibilityEvent) {
        if (!listeningWindows) return
        val type = e.eventType
        if (type != AccessibilityEvent.TYPE_WINDOWS_CHANGED && type != AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED) return
        val json = JSONObject()
            .put("type", type)
            .put("window_id", e.windowId)
            .putOpt("package", e.packageName?.toString())
            .putOpt("class_name", e.className?.toString())
        if (Build.VERSION.SDK_INT >= 28) json.put("changes", e.windowChanges)
        if (Build.VERSION.SDK_INT >= 30) json.put("display_id", e.displayId)
        nativeWindowEvent(json.toString())
    }

    fun connected(s: RustAccessibilityService) {
        val was = service
        service = s
        if (was == null && listening) nativeEnabled(true)
    }

    fun disconnected(s: RustAccessibilityService) {
        // onUnbind and onDestroy both land here; report the first only.
        if (service !== s) return
        service = null
        if (listening) nativeEnabled(false)
    }

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
        } catch (e: IllegalArgumentException) {
            error("invalid_argument", e.message)
        } catch (t: Throwable) {
            error("platform", "${t.javaClass.simpleName}: ${t.message}")
        }
    }

    private fun error(code: String, message: String?): String =
        JSONObject().put("error", code).put("message", message ?: "").toString()

    private fun requireService(): RustAccessibilityService =
        service ?: throw Failure("not_enabled", "the accessibility service is not switched on")

    private fun requireApi(level: Int, what: String) {
        if (Build.VERSION.SDK_INT < level) {
            throw Failure("unsupported", "$what needs API $level; this device is API ${Build.VERSION.SDK_INT}")
        }
    }

    private fun dispatch(ctx: Context, method: String, args: JSONObject, token: Long): Any? = when (method) {
        "is_enabled" -> service != null
        "listen" -> {
            val on = args.getBoolean("on")
            when (args.getString("what")) {
                "enabled" -> listening = on
                "windows" -> listeningWindows = on
                else -> throw Failure("invalid_argument", "nothing to listen for named ${args.getString("what")}")
            }
            null
        }
        "open_settings" -> {
            ctx.startActivity(
                Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            )
            null
        }
        "screenshot" -> screenshot(args, token)
        "gesture" -> gesture(args, token)
        "global_action" -> globalAction(args.getString("action"))
        "set_focused_text" -> setFocusedText(args.getString("text"))
        "find" -> find(args, token)
        "click" -> click(args, token)
        "windows" -> windows(token)
        else -> throw Failure("platform", "unknown method $method")
    }

    private fun defaultDisplay(s: Context): Display =
        (s.getSystemService(Context.DISPLAY_SERVICE) as DisplayManager).getDisplay(Display.DEFAULT_DISPLAY)

    /** The default display's size in its current rotation: the space gestures are dispatched in. */
    private fun screenSize(s: Context): Point {
        val size = Point()
        @Suppress("DEPRECATION")
        defaultDisplay(s).getRealSize(size)
        return size
    }

    private fun screenshot(args: JSONObject, token: Long): Any {
        requireApi(30, "screenshot")
        val s = requireService()
        val jpegQuality = if (args.getString("format") == "jpeg") args.getInt("quality") else -1
        // A whole-display screenshot does not fail over a FLAG_SECURE window:
        // Android blanks the window and reports success, and a black frame
        // reads as a hung device. Only takeScreenshotOfWindow (API 34) tells,
        // so the active window is probed with it alongside the display
        // capture (in parallel, so the probe adds no latency), and the frame
        // is dropped if it is secure. Below 34 a secure window still arrives
        // black. Needs flagRetrieveInteractiveWindows in the service config,
        // or every probe fails with INVALID_WINDOW.
        val activeWindow = if (Build.VERSION.SDK_INT >= 34) s.rootInActiveWindow?.windowId else null
        val capture = Capture(s, jpegQuality, token, probing = activeWindow != null)
        s.takeScreenshot(Display.DEFAULT_DISPLAY, worker, object : AccessibilityService.TakeScreenshotCallback {
            override fun onSuccess(result: AccessibilityService.ScreenshotResult) = capture.display(result, 0)
            override fun onFailure(errorCode: Int) = capture.display(null, errorCode)
        })
        if (activeWindow != null) {
            s.takeScreenshotOfWindow(activeWindow, worker, object : AccessibilityService.TakeScreenshotCallback {
                override fun onSuccess(result: AccessibilityService.ScreenshotResult) {
                    result.hardwareBuffer.close()
                    capture.probed(secure = false)
                }

                override fun onFailure(errorCode: Int) {
                    // SECURE_WINDOW (5) is the documented answer, but the API
                    // 34 emulator answers INVALID_WINDOW (6) for a secure
                    // window (Chrome incognito) that is still on screen. 6
                    // also means the window closed in between, so it counts
                    // as secure only while the window is still listed. Any
                    // other failure says nothing about secure content.
                    capture.probed(
                        secure = errorCode == 5 || (errorCode == 6 && s.windows.any { it.id == activeWindow })
                    )
                }
            })
        }
        return Pending
    }

    /**
     * One [screenshot] request: the display capture and the secure-window
     * probe, joined. Both callbacks run on [worker], a single thread, so the
     * fields need no lock; the reply goes out once both have reported.
     */
    private class Capture(
        val s: RustAccessibilityService,
        val jpegQuality: Int,
        val token: Long,
        probing: Boolean,
    ) {
        private var probeDone = !probing
        private var secure = false
        private var displayDone = false
        private var replied = false
        private var result: AccessibilityService.ScreenshotResult? = null
        private var errorCode = 0

        fun probed(secure: Boolean) {
            probeDone = true
            this.secure = secure
            finish()
        }

        fun display(result: AccessibilityService.ScreenshotResult?, errorCode: Int) {
            displayDone = true
            this.result = result
            this.errorCode = errorCode
            if (!probeDone) {
                worker.schedule({ probeDone = true; finish() }, PROBE_GRACE_MS, TimeUnit.MILLISECONDS)
            }
            finish()
        }

        private fun finish() {
            // A probe answering after the grace period finds this replied.
            if (!probeDone || !displayDone || replied) return
            replied = true
            val shot = result
            when {
                secure -> {
                    shot?.hardwareBuffer?.close()
                    nativeResult(token, screenshotFailure(5))
                }
                shot == null -> nativeResult(token, screenshotFailure(errorCode))
                else -> encode(s, shot, jpegQuality, token)
            }
        }
    }

    private fun encode(s: RustAccessibilityService, result: AccessibilityService.ScreenshotResult, jpegQuality: Int, token: Long) {
        try {
            val buffer = result.hardwareBuffer
            val hardware = Bitmap.wrapHardwareBuffer(buffer, result.colorSpace)
                ?: throw Failure("platform", "could not wrap the screenshot buffer")
            val bitmap = hardware.copy(Bitmap.Config.ARGB_8888, false)
            hardware.recycle()
            buffer.close()
            val bytes = if (jpegQuality >= 0) {
                val out = ByteArrayOutputStream()
                bitmap.compress(Bitmap.CompressFormat.JPEG, jpegQuality, out)
                out.toByteArray()
            } else {
                // ARGB_8888 is laid out R, G, B, A in memory.
                val pixels = ByteBuffer.allocate(bitmap.byteCount)
                bitmap.copyPixelsToBuffer(pixels)
                pixels.array()
            }
            val rotation = defaultDisplay(s).rotation
            nativeScreenshot(token, bitmap.width, bitmap.height, rotation, bytes)
            bitmap.recycle()
        } catch (f: Failure) {
            nativeResult(token, error(f.code, f.message))
        } catch (t: Throwable) {
            nativeResult(token, error("platform", "${t.javaClass.simpleName}: ${t.message}"))
        }
    }

    /** `AccessibilityService.ERROR_TAKE_SCREENSHOT_*`, as literals: some are newer than the compile SDK names them. */
    private fun screenshotFailure(code: Int): String = when (code) {
        2 -> error("not_enabled", "the service may not take screenshots (NO_ACCESSIBILITY_ACCESS)")
        3 -> error("too_soon", "")
        5 -> error("secure_window", "a secure window is on screen")
        else -> error("platform", "takeScreenshot failed with error $code")
    }

    private fun gesture(args: JSONObject, token: Long): Any {
        val s = requireService()
        return stroke(
            s,
            args.getDouble("x1").toFloat(),
            args.getDouble("y1").toFloat(),
            args.getDouble("x2").toFloat(),
            args.getDouble("y2").toFloat(),
            args.getLong("duration_ms"),
            token,
        )
    }

    /** Dispatch one stroke; its result goes to [nativeResult] under [token]. */
    private fun stroke(s: RustAccessibilityService, x1: Float, y1: Float, x2: Float, y2: Float, durationMs: Long, token: Long): Any {
        val size = screenSize(s)
        for ((x, y) in listOf(x1 to y1, x2 to y2)) {
            if (x >= size.x || y >= size.y) {
                throw Failure("invalid_argument", "($x, $y) is outside the ${size.x} × ${size.y} screen")
            }
        }
        val path = Path().apply {
            moveTo(x1, y1)
            if (x1 != x2 || y1 != y2) lineTo(x2, y2)
        }
        val stroke = GestureDescription.StrokeDescription(path, 0, durationMs)
        val accepted = s.dispatchGesture(
            GestureDescription.Builder().addStroke(stroke).build(),
            object : AccessibilityService.GestureResultCallback() {
                override fun onCompleted(gestureDescription: GestureDescription?) {
                    nativeResult(token, JSONObject().put("ok", JSONObject.NULL).toString())
                }

                override fun onCancelled(gestureDescription: GestureDescription?) {
                    nativeResult(token, error("gesture_cancelled", "the gesture was cancelled"))
                }
            },
            main,
        )
        if (!accepted) throw Failure("refused", "the system did not accept the gesture")
        return Pending
    }

    private fun globalAction(name: String): Any? {
        val s = requireService()
        val action = when (name) {
            "back" -> AccessibilityService.GLOBAL_ACTION_BACK
            "home" -> AccessibilityService.GLOBAL_ACTION_HOME
            "recents" -> AccessibilityService.GLOBAL_ACTION_RECENTS
            "notifications" -> AccessibilityService.GLOBAL_ACTION_NOTIFICATIONS
            "quick_settings" -> AccessibilityService.GLOBAL_ACTION_QUICK_SETTINGS
            "lock_screen" -> {
                requireApi(28, "GlobalAction::LockScreen")
                AccessibilityService.GLOBAL_ACTION_LOCK_SCREEN
            }
            else -> throw Failure("invalid_argument", "unknown global action $name")
        }
        if (!s.performGlobalAction(action)) throw Failure("refused", "the system refused global action $name")
        return null
    }

    private fun setFocusedText(text: String): Any? {
        val s = requireService()
        val node = s.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
            ?: throw Failure("no_focused_input", "no input field has focus")
        if (!node.isEditable) throw Failure("refused", "the focused node is not editable")
        val arguments = Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
        }
        if (!node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, arguments)) {
            throw Failure("refused", "the focused field refused the text")
        }
        return null
    }

    /**
     * A query from Rust's `Query::to_json`. Text compares ignore case: a
     * button styled in capitals reports its text as written.
     */
    private class Query(o: JSONObject) {
        /** A criterion: a string, or `{"exact": s}` / `{"contains": s}` for text; absent is null. */
        private fun criterion(o: JSONObject, key: String): Any? = if (o.isNull(key)) null else o.get(key)

        val viewId = criterion(o, "view_id") as String?
        val text = criterion(o, "text") as JSONObject?
        val description = criterion(o, "description") as JSONObject?
        val pkg = criterion(o, "package") as String?
        val className = criterion(o, "class_name") as String?

        fun matches(n: AccessibilityNodeInfo): Boolean {
            if (pkg != null && n.packageName?.toString() != pkg) return false
            if (viewId != null) {
                val id = n.viewIdResourceName ?: return false
                // "android:id/button1" exactly; "button1" in any package.
                if (if (':' in viewId) id != viewId else !id.endsWith(":id/$viewId")) return false
            }
            if (className != null) {
                val c = n.className?.toString() ?: return false
                if (if ('.' in className) c != className else c.substringAfterLast('.') != className) return false
            }
            if (text != null && !textMatches(text, n.text)) return false
            if (description != null && !textMatches(description, n.contentDescription)) return false
            return true
        }

        private fun textMatches(m: JSONObject, value: CharSequence?): Boolean {
            val v = value?.toString() ?: return false
            return if (m.has("exact")) v.equals(m.getString("exact"), ignoreCase = true)
            else v.contains(m.getString("contains"), ignoreCase = true)
        }
    }

    /**
     * `find` and `wait_for`: search every interactive window on [finder].
     * Without `timeout_ms` one search replies with what it found, empty
     * included; with it the search repeats every `poll_ms` until something
     * matches or the time is up (`timeout`).
     */
    private fun find(args: JSONObject, token: Long): Any {
        requireService()
        val query = Query(args.getJSONObject("query"))
        val pollMs = args.getLong("poll_ms")
        synchronized(nodes) { nodeHandles = args.getInt("node_handles") }
        val waiting = args.has("timeout_ms")
        val timeoutMs = if (waiting) args.getLong("timeout_ms") else 0L
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeoutMs)
        finder.execute(object : Runnable {
            override fun run() {
                try {
                    val s = requireService()
                    // A zero timeout checks once, so its one walk runs to the end.
                    val found = search(s, query, if (timeoutMs > 0) deadline else null)
                    when {
                        found.length() > 0 || !waiting -> nativeResult(token, JSONObject().put("ok", found).toString())
                        System.nanoTime() >= deadline -> nativeResult(token, error("timeout", "nothing matched before the timeout"))
                        else -> finder.schedule(this, pollMs, TimeUnit.MILLISECONDS)
                    }
                } catch (f: Failure) {
                    nativeResult(token, error(f.code, f.message))
                } catch (t: Throwable) {
                    nativeResult(token, error("platform", "${t.javaClass.simpleName}: ${t.message}"))
                }
            }
        })
        return Pending
    }

    /**
     * Every match in every interactive window, depth first in tree order.
     * With a [deadline] (`wait_for`) the walk stops once it passes, with
     * what it has matched so far: each step is a call into the window's
     * app, which Android lets block for seconds while that app's UI thread
     * is busy (SystemUI closing a dialog), so a check only between
     * searches would let one search overrun the timeout by many seconds.
     */
    private fun search(s: RustAccessibilityService, query: Query, deadline: Long?): JSONArray {
        val out = JSONArray()
        for (window in s.windows) {
            val root = window.root ?: continue
            val stack = ArrayDeque<AccessibilityNodeInfo>()
            stack.addLast(root)
            while (stack.isNotEmpty()) {
                if (deadline != null && System.nanoTime() >= deadline) return out
                val n = stack.removeLast()
                if (query.matches(n)) out.put(describe(n, remember(n)))
                // Pushed last-first so the first child is visited next.
                for (i in n.childCount - 1 downTo 0) n.getChild(i)?.let { stack.addLast(it) }
            }
        }
        return out
    }

    private fun remember(n: AccessibilityNodeInfo): Long = synchronized(nodes) {
        val handle = nextNode++
        nodes[handle] = n
        handle
    }

    private fun describe(n: AccessibilityNodeInfo, handle: Long): JSONObject {
        val r = Rect()
        n.getBoundsInScreen(r)
        return JSONObject()
            .put("handle", handle)
            .put("window_id", n.windowId)
            .put("bounds", JSONArray().put(r.left).put(r.top).put(r.right).put(r.bottom))
            .putOpt("text", n.text?.toString())
            .putOpt("description", n.contentDescription?.toString())
            .putOpt("view_id", n.viewIdResourceName)
            .putOpt("package", n.packageName?.toString())
            .putOpt("class_name", n.className?.toString())
            .put("clickable", n.isClickable)
            .put("enabled", n.isEnabled)
    }

    /**
     * `click`: `ACTION_CLICK` on the node when it takes one, else a tap at
     * the centre of its bounds as they are now. On [finder]: `refresh` and
     * `performAction` are calls into the app that owns the node.
     */
    private fun click(args: JSONObject, token: Long): Any {
        requireService()
        val handle = args.getLong("node")
        val tapMs = args.getLong("tap_ms")
        finder.execute {
            try {
                val s = requireService()
                val n = synchronized(nodes) { nodes[handle] }
                    ?: throw Failure("stale_node", "the element was found too long ago; find it again")
                // false: the view is gone, or its window is.
                if (!n.refresh()) throw Failure("stale_node", "the element is no longer on screen")
                if (n.isClickable && n.isEnabled && n.performAction(AccessibilityNodeInfo.ACTION_CLICK)) {
                    nativeResult(token, JSONObject().put("ok", JSONObject.NULL).toString())
                } else {
                    val r = Rect()
                    n.getBoundsInScreen(r)
                    if (r.isEmpty) throw Failure("refused", "the element takes no click and has no area on screen to tap")
                    stroke(s, r.exactCenterX(), r.exactCenterY(), r.exactCenterX(), r.exactCenterY(), tapMs, token)
                }
            } catch (f: Failure) {
                nativeResult(token, error(f.code, f.message))
            } catch (t: Throwable) {
                nativeResult(token, error("platform", "${t.javaClass.simpleName}: ${t.message}"))
            }
        }
        return Pending
    }

    /** `windows`: the default display's windows, top of the stack first. On [finder]: each `root` is a call into its app. */
    private fun windows(token: Long): Any {
        requireService()
        finder.execute {
            try {
                val out = JSONArray()
                for (w in requireService().windows) out.put(describe(w))
                nativeResult(token, JSONObject().put("ok", out).toString())
            } catch (f: Failure) {
                nativeResult(token, error(f.code, f.message))
            } catch (t: Throwable) {
                nativeResult(token, error("platform", "${t.javaClass.simpleName}: ${t.message}"))
            }
        }
        return Pending
    }

    private fun describe(w: AccessibilityWindowInfo): JSONObject {
        val r = Rect()
        w.getBoundsInScreen(r)
        val json = JSONObject()
            .put("id", w.id)
            .put("type", w.type)
            .put("layer", w.layer)
            .put("bounds", JSONArray().put(r.left).put(r.top).put(r.right).put(r.bottom))
            .putOpt("package", w.root?.packageName?.toString())
            .putOpt("title", w.title?.toString())
            .put("focused", w.isFocused)
            .put("active", w.isActive)
        if (Build.VERSION.SDK_INT >= 26) json.put("picture_in_picture", w.isInPictureInPictureMode)
        if (Build.VERSION.SDK_INT >= 30) json.put("display_id", w.displayId)
        return json
    }
}
