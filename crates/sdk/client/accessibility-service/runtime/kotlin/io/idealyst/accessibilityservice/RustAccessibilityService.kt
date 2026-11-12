package io.idealyst.accessibilityservice

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Path
import android.graphics.Point
import android.hardware.display.DisplayManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.view.Display
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.json.JSONObject

/**
 * The app's accessibility service. The system binds it once someone
 * switches it on in Settings > Accessibility, and it then does what Rust
 * asks through [RustAccessibility]. It reads no accessibility events.
 */
class RustAccessibilityService : AccessibilityService() {
    override fun onServiceConnected() {
        super.onServiceConnected()
        RustAccessibility.connected(this)
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {}

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
 * `no_focused_input`, `invalid_argument`, `unsupported`, `platform`.
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

    private class Failure(val code: String, message: String) : Exception(message)

    private object Pending

    @JvmStatic external fun nativeResult(token: Long, json: String)

    @JvmStatic external fun nativeScreenshot(token: Long, width: Int, height: Int, rotation: Int, data: ByteArray)

    @JvmStatic external fun nativeEnabled(enabled: Boolean)

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
        "attach" -> { listening = true; null }
        "detach" -> { listening = false; null }
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
        val size = screenSize(s)
        val x1 = args.getDouble("x1").toFloat()
        val y1 = args.getDouble("y1").toFloat()
        val x2 = args.getDouble("x2").toFloat()
        val y2 = args.getDouble("y2").toFloat()
        for ((x, y) in listOf(x1 to y1, x2 to y2)) {
            if (x >= size.x || y >= size.y) {
                throw Failure("invalid_argument", "($x, $y) is outside the ${size.x} × ${size.y} screen")
            }
        }
        val path = Path().apply {
            moveTo(x1, y1)
            if (x1 != x2 || y1 != y2) lineTo(x2, y2)
        }
        val stroke = GestureDescription.StrokeDescription(path, 0, args.getLong("duration_ms"))
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
}
