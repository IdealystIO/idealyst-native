package io.idealyst.location

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.location.LocationRequest
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The Android half of the `location` SDK: one fix ([current]) and a stream of
 * them ([startWatch]), from the framework `LocationManager` (no Play
 * Services).
 *
 * Every call uses the application context, so it works with no Activity: from
 * a background process kept alive by a service. What Android itself asks of a
 * background process (`ACCESS_BACKGROUND_LOCATION`) is in the crate's README.
 *
 * Results come back on the main thread through the `native*` exports in
 * `src/android.rs`.
 */
object RustLocation {
    // Immediate answers of [current] and [startWatch]; mirrored in
    // `src/android_fix.rs`.
    private const val STARTED = 0
    private const val NOT_AUTHORIZED = 1
    private const val SERVICES_OFF = 2
    private const val NO_PROVIDER = 3

    /** `LocationManager.FUSED_PROVIDER` (API 31). */
    private const val FUSED = "fused"

    /** How long the best provider has before the others are asked too. */
    private const val WIDEN_AFTER_MS = 10_000L

    private val main = Handler(Looper.getMainLooper())
    private val requests = ConcurrentHashMap<Long, Request>()
    private val watches = ConcurrentHashMap<Long, Watch>()

    /** The fix [token] asked for. Absent fields are NaN. */
    @JvmStatic
    external fun nativeOnFix(
        token: Long, latitude: Double, longitude: Double, accuracy: Double,
        altitude: Double, heading: Double, speed: Double, timeMs: Long,
    )

    /** [token] got no fix. */
    @JvmStatic
    external fun nativeOnNoFix(token: Long, reason: String)

    /** One update of watch [id]. Absent fields are NaN. */
    @JvmStatic
    external fun nativeOnUpdate(
        id: Long, latitude: Double, longitude: Double, accuracy: Double,
        altitude: Double, heading: Double, speed: Double, timeMs: Long,
    )

    /** Whether Location is on for the device. */
    @JvmStatic
    fun servicesEnabled(context: Context): Boolean = enabled(context.applicationContext)

    /**
     * Ask for one fix newer than this call, within [timeoutMs]; after that,
     * the newest one the device already has. [STARTED] answers through
     * [nativeOnFix] or [nativeOnNoFix].
     */
    @JvmStatic
    fun current(context: Context, token: Long, timeoutMs: Long): Int {
        val ctx = context.applicationContext
        if (!permitted(ctx)) return NOT_AUTHORIZED
        if (!enabled(ctx)) return SERVICES_OFF
        val lm = manager(ctx)
        val request = Request(lm, token)
        requests[token] = request
        val refused = request.start(timeoutMs) ?: return STARTED
        requests.remove(token)
        return refused
    }

    /** Give up on [token]; nothing is reported for it any more. */
    @JvmStatic
    fun cancel(token: Long) {
        requests.remove(token)?.stop()
    }

    /** Stream updates to [nativeOnUpdate] until [stopWatch]. */
    @JvmStatic
    fun startWatch(context: Context, id: Long): Int {
        val ctx = context.applicationContext
        if (!permitted(ctx)) return NOT_AUTHORIZED
        val lm = manager(ctx)
        // Present providers, on or not: a watch started while Location is off
        // begins when it is turned on.
        val providers = tiers(lm) { true }.firstOrNull() ?: return NO_PROVIDER
        val watch = Watch(lm, id)
        if (!subscribe(lm, providers, WATCH_INTERVAL_MS, watch)) return NOT_AUTHORIZED
        watches[id] = watch
        return STARTED
    }

    @JvmStatic
    fun stopWatch(id: Long) {
        watches.remove(id)?.stop()
    }

    private const val WATCH_INTERVAL_MS = 1_000L

    private fun manager(ctx: Context) = ctx.getSystemService(Context.LOCATION_SERVICE) as LocationManager

    /** Either grant will do. Granted at install below API 23. */
    private fun permitted(ctx: Context): Boolean =
        Build.VERSION.SDK_INT < 23 || listOf(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION)
            .any { ctx.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }

    private fun enabled(ctx: Context): Boolean =
        if (Build.VERSION.SDK_INT >= 28) {
            manager(ctx).isLocationEnabled
        } else {
            @Suppress("DEPRECATION")
            Settings.Secure.getInt(ctx.contentResolver, Settings.Secure.LOCATION_MODE, 0) != 0
        }

    /**
     * The providers to use, best first, as tiers asked one after the other:
     * the fused one, which chooses between the rest itself, then GPS and
     * network together (GPS never fixes indoors, and a tablet may have none).
     */
    private fun tiers(lm: LocationManager, use: (String) -> Boolean): List<List<String>> {
        val present = lm.allProviders
        val fused = listOf(FUSED).filter { Build.VERSION.SDK_INT >= 31 && it in present && use(it) }
        val single = listOf(LocationManager.GPS_PROVIDER, LocationManager.NETWORK_PROVIDER)
            .filter { it in present && use(it) }
        return listOf(fused, single).filter { it.isNotEmpty() }
    }

    /**
     * Ask [providers] for updates on the main thread. `false` when none may be
     * asked (GPS refuses an app with coarse location only, below API 31).
     */
    private fun subscribe(
        lm: LocationManager, providers: List<String>, intervalMs: Long, listener: LocationListener,
    ): Boolean = providers.map { provider ->
        try {
            if (Build.VERSION.SDK_INT >= 31) {
                val request = LocationRequest.Builder(intervalMs)
                    .setQuality(LocationRequest.QUALITY_HIGH_ACCURACY)
                    .build()
                lm.requestLocationUpdates(provider, request, { main.post(it) }, listener)
            } else {
                lm.requestLocationUpdates(provider, intervalMs, 0f, listener, Looper.getMainLooper())
            }
            true
        } catch (e: SecurityException) {
            false
        } catch (e: IllegalArgumentException) {
            // The provider went away since it was listed.
            false
        }
    }.any { it }

    /** The newest fix any provider has, however old. */
    private fun cached(lm: LocationManager): Location? = lm.allProviders
        .mapNotNull {
            try {
                lm.getLastKnownLocation(it)
            } catch (e: SecurityException) {
                null
            } catch (e: IllegalArgumentException) {
                null
            }
        }
        .maxByOrNull { it.elapsedRealtimeNanos }

    /**
     * A request for one fix. Updates rather than
     * `LocationManager.getCurrentLocation` (API 30): that one may answer with a
     * fix up to ten seconds older than the call, and does not exist below 30.
     */
    private class Request(private val lm: LocationManager, private val token: Long) : Listener() {
        private val startedNanos = SystemClock.elapsedRealtimeNanos()
        private val done = AtomicBoolean(false)
        private var rest: List<String> = emptyList()
        private val widen = Runnable { if (!done.get()) subscribe(lm, rest, 0L, this) }
        private val expire = Runnable { finish(cached(lm), "no fix in time, and none known from before") }

        /** Begin. `null` when started, else why not. */
        fun start(timeoutMs: Long): Int? {
            val tiers = tiers(lm) { lm.isProviderEnabled(it) }
            if (tiers.isEmpty()) {
                // Location is on with nothing to ask: what is known, if anything.
                val known = cached(lm) ?: return NO_PROVIDER
                main.post { finish(known, "") }
                return null
            }
            rest = tiers.drop(1).flatten()
            // A tier that may not be asked is no reason to wait for it.
            if (!subscribe(lm, tiers.first(), 0L, this)) {
                if (rest.isEmpty() || !subscribe(lm, rest, 0L, this)) return NOT_AUTHORIZED
                rest = emptyList()
            }
            if (rest.isNotEmpty()) main.postDelayed(widen, WIDEN_AFTER_MS.coerceAtMost(timeoutMs / 2))
            main.postDelayed(expire, timeoutMs)
            return null
        }

        override fun onLocationChanged(location: Location) {
            // A provider may open with the fix it already had.
            if (location.elapsedRealtimeNanos >= startedNanos) finish(location, "")
        }

        private fun finish(location: Location?, reason: String) {
            if (!done.compareAndSet(false, true)) return
            release()
            requests.remove(token)
            if (location == null) {
                nativeOnNoFix(token, reason)
            } else {
                nativeOnFix(
                    token, location.latitude, location.longitude, accuracy(location),
                    altitude(location), heading(location), speed(location), location.time,
                )
            }
        }

        fun stop() {
            if (done.compareAndSet(false, true)) release()
        }

        private fun release() {
            main.removeCallbacks(widen)
            main.removeCallbacks(expire)
            lm.removeUpdates(this)
        }
    }

    private class Watch(private val lm: LocationManager, private val id: Long) : Listener() {
        override fun onLocationChanged(location: Location) {
            // One already posted when the watch was stopped.
            if (!watches.containsKey(id)) return
            nativeOnUpdate(
                id, location.latitude, location.longitude, accuracy(location),
                altitude(location), heading(location), speed(location), location.time,
            )
        }

        fun stop() = lm.removeUpdates(this)
    }

    /** The callbacks that have no default below API 30. */
    private abstract class Listener : LocationListener {
        override fun onProviderEnabled(provider: String) {}
        override fun onProviderDisabled(provider: String) {}

        @Deprecated("called below API 29")
        override fun onStatusChanged(provider: String?, status: Int, extras: Bundle?) {}

        protected fun accuracy(l: Location) = if (l.hasAccuracy()) l.accuracy.toDouble() else Double.NaN
        protected fun altitude(l: Location) = if (l.hasAltitude()) l.altitude else Double.NaN
        protected fun heading(l: Location) = if (l.hasBearing()) l.bearing.toDouble() else Double.NaN
        protected fun speed(l: Location) = if (l.hasSpeed()) l.speed.toDouble() else Double.NaN
    }
}
