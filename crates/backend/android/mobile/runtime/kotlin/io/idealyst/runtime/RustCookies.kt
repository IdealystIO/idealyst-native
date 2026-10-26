package io.idealyst.runtime

import android.content.Context
import android.os.Build
import java.net.CookieHandler
import java.net.CookieManager
import java.net.CookiePolicy
import java.net.CookieStore
import java.net.HttpCookie
import java.net.URI
import org.json.JSONArray
import org.json.JSONObject

/**
 * The process's cookie jar: what `net`'s `HttpURLConnection` transport
 * stores `Set-Cookie` into and sends `Cookie` from.
 *
 * `HttpURLConnection` keeps cookies only when a default [CookieHandler]
 * is installed, and nothing installs one — so without this every
 * `Set-Cookie` was read and dropped. A server that authenticates by
 * cookie (an httpOnly session, a kiosk's device cookie) then never sees
 * its credential again: the request after a sign-in or an enrolment
 * arrives anonymous.
 *
 * iOS gets this from `NSURLSession`'s shared cookie storage, which keeps
 * cookies per origin and persists those with an expiry. This mirrors it:
 * matching, paths and expiry are the JDK's own `CookieManager` store, and
 * cookies with a `Max-Age` / `Expires` are also written to
 * SharedPreferences and restored at the next launch. Session cookies stay
 * in memory, as in a browser.
 */
object RustCookies {
    /** Install the persistent jar as the default handler, once. An app that
     *  installed its own handler first keeps it. */
    @JvmStatic
    fun install(context: Context) {
        if (CookieHandler.getDefault() != null) return
        val store = PersistentCookieStore(context.applicationContext)
        CookieHandler.setDefault(CookieManager(store, CookiePolicy.ACCEPT_ORIGINAL_SERVER))
    }
}

/**
 * A [CookieStore] that delegates every lookup to the JDK's in-memory store
 * and mirrors the persistent cookies to SharedPreferences.
 *
 * Each persistent cookie is saved with the origin it was set for (the
 * in-memory store indexes a cookie without a `Domain` by that host) and
 * its absolute expiry, so a restored cookie keeps the lifetime the server
 * gave it rather than restarting its `Max-Age`.
 */
private class PersistentCookieStore(context: Context) : CookieStore {
    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    private val memory: CookieStore = CookieManager().cookieStore
    private val saved = LinkedHashMap<String, JSONObject>()

    init {
        restore()
    }

    override fun add(uri: URI?, cookie: HttpCookie) {
        // `HttpCookie.parse` reads any `Max-Age` as an RFC 2965 version-1
        // cookie, and `CookieManager` sends those as
        // `$Version="1"; name="value"` — quoted, which a server reading
        // the RFC 6265 `name=value` that browsers send takes as part of
        // the value. Version 0 sends the plain pair.
        cookie.version = 0
        memory.add(uri, cookie)
        synchronized(saved) {
            val key = keyOf(uri, cookie)
            // Max-Age 0 / a past Expires deletes; a session cookie of the
            // same name replaces the persistent one in memory, so it must
            // not come back from disk either.
            if (cookie.maxAge > 0 && uri != null) {
                saved[key] = encode(uri, cookie)
            } else {
                saved.remove(key)
            }
            persist()
        }
    }

    override fun get(uri: URI): List<HttpCookie> = memory.get(uri)

    override fun getCookies(): List<HttpCookie> = memory.cookies

    override fun getURIs(): List<URI> = memory.urIs

    override fun remove(uri: URI?, cookie: HttpCookie): Boolean {
        val removed = memory.remove(uri, cookie)
        synchronized(saved) {
            if (saved.remove(keyOf(uri, cookie)) != null) persist()
        }
        return removed
    }

    override fun removeAll(): Boolean {
        val removed = memory.removeAll()
        synchronized(saved) {
            saved.clear()
            persist()
        }
        return removed
    }

    private fun restore() {
        val raw = prefs.getString(KEY, null) ?: return
        val now = System.currentTimeMillis()
        val entries = try {
            JSONArray(raw)
        } catch (_: Exception) {
            return
        }
        for (i in 0 until entries.length()) {
            val e = entries.optJSONObject(i) ?: continue
            val remaining = (e.optLong("expires") - now) / 1000
            if (remaining <= 0) continue
            val cookie = HttpCookie(e.getString("name"), e.getString("value")).apply {
                domain = e.optString("domain").ifEmpty { null }
                path = e.optString("path").ifEmpty { null }
                secure = e.optBoolean("secure")
                // The constructor defaults to version 1; see `add`.
                version = 0
                maxAge = remaining
                if (Build.VERSION.SDK_INT >= 24) isHttpOnly = e.optBoolean("httpOnly")
            }
            val uri = try {
                URI(e.getString("uri"))
            } catch (_: Exception) {
                continue
            }
            memory.add(uri, cookie)
            saved[keyOf(uri, cookie)] = e
        }
    }

    /** Callers hold the `saved` lock. */
    private fun persist() {
        val arr = JSONArray()
        saved.values.forEach { arr.put(it) }
        prefs.edit().putString(KEY, arr.toString()).apply()
    }

    private fun encode(uri: URI, cookie: HttpCookie): JSONObject = JSONObject().apply {
        put("uri", origin(uri).toString())
        put("name", cookie.name)
        put("value", cookie.value)
        put("domain", cookie.domain ?: "")
        put("path", cookie.path ?: "")
        put("secure", cookie.secure)
        put("expires", System.currentTimeMillis() + cookie.maxAge * 1000)
        if (Build.VERSION.SDK_INT >= 24) put("httpOnly", cookie.isHttpOnly)
    }

    /** One cookie per name, domain (or setting host) and path — the
     *  identity RFC 6265 replaces on. */
    private fun keyOf(uri: URI?, cookie: HttpCookie): String =
        "${cookie.name};${cookie.domain ?: uri?.host ?: ""};${cookie.path ?: ""}"

    /** Scheme and host only: the store indexes by host, and a request URI's
     *  path or query has no business on disk. */
    private fun origin(uri: URI): URI = URI(uri.scheme, uri.host, null, null)

    companion object {
        private const val PREFS = "io.idealyst.cookies"
        private const val KEY = "cookies"
    }
}
