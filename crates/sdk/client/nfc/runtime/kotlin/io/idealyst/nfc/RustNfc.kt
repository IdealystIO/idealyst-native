package io.idealyst.nfc

import android.app.Activity
import android.content.Context
import android.nfc.FormatException
import android.nfc.NdefMessage
import android.nfc.NfcAdapter
import android.nfc.Tag
import android.nfc.TagLostException
import android.nfc.tech.IsoDep
import android.nfc.tech.MifareUltralight
import android.nfc.tech.Ndef
import android.nfc.tech.NdefFormatable
import android.nfc.tech.NfcA
import android.nfc.tech.NfcB
import android.nfc.tech.NfcF
import android.nfc.tech.NfcV
import android.nfc.tech.TagTechnology
import android.os.Handler
import android.os.Looper
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors

/**
 * Bridges `NfcAdapter` reader mode to the `nfc` SDK (`src/android.rs`).
 *
 * One reader session at a time. [begin] enables reader mode on the Activity;
 * each discovered tag is stored under a fresh handle and reported through
 * [nativeTagDiscovered] (on the NFC binder thread). Tag I/O blocks, so
 * [readNdef] / [writeNdef] / [transceive] run on one background thread and
 * report through [nativeOpResult]; the single thread also serializes access
 * to the connected technologies.
 *
 * A technology stays connected between operations: closing an `IsoDep`
 * connection deselects the card's application, so an APDU sequence
 * (SELECT, then READ) must share one connection. Android allows one
 * connected technology per tag, so switching technology closes the
 * previous one first.
 *
 * Shipped from the `nfc` SDK crate via
 * `[package.metadata.idealyst.android].runtime_kotlin`.
 */
object RustNfc {
    // Status codes shared with `src/android.rs`.
    private const val OK = 0
    private const val TAG_LOST = 1
    private const val NOT_NDEF = 2
    private const val READ_ONLY = 3
    private const val TOO_LARGE = 4
    private const val ERROR = 5
    private const val NO_RAW_TECH = 6

    // `begin` results.
    private const val BEGUN = 0
    private const val UNSUPPORTED = 1
    private const val DISABLED = 2

    private val io = Executors.newSingleThreadExecutor { r -> Thread(r, "idealyst-nfc-io") }
    private val tags = ConcurrentHashMap<Long, Tag>()
    // Touched only on the `io` thread.
    private val connected = HashMap<Long, TagTechnology>()
    private val lock = Any()
    private var activeSession = 0L
    private var nextHandle = 1L

    @JvmStatic
    fun availability(context: Context): Int {
        val adapter = NfcAdapter.getDefaultAdapter(context) ?: return UNSUPPORTED
        return if (adapter.isEnabled) BEGUN else DISABLED
    }

    @JvmStatic
    fun begin(activity: Activity, session: Long): Int {
        val adapter = NfcAdapter.getDefaultAdapter(activity) ?: return UNSUPPORTED
        if (!adapter.isEnabled) return DISABLED
        synchronized(lock) { activeSession = session }
        val flags = NfcAdapter.FLAG_READER_NFC_A or NfcAdapter.FLAG_READER_NFC_B or
            NfcAdapter.FLAG_READER_NFC_F or NfcAdapter.FLAG_READER_NFC_V
        onMain {
            adapter.enableReaderMode(activity, { tag -> discovered(session, tag) }, flags, null)
        }
        return BEGUN
    }

    @JvmStatic
    fun end(activity: Activity, session: Long) {
        synchronized(lock) {
            if (activeSession != session) return
            activeSession = 0L
            tags.clear()
        }
        io.execute {
            for (tech in connected.values) closeQuietly(tech)
            connected.clear()
        }
        val adapter = NfcAdapter.getDefaultAdapter(activity) ?: return
        onMain {
            try {
                adapter.disableReaderMode(activity)
            } catch (_: IllegalStateException) {
                // The Activity is no longer resumed; reader mode is already off.
            }
        }
    }

    /** Forget one tag (its Rust handle was dropped). */
    @JvmStatic
    fun release(handle: Long) {
        tags.remove(handle)
        io.execute { connected.remove(handle)?.let(::closeQuietly) }
    }

    @JvmStatic
    fun readNdef(handle: Long, op: Long) = run(handle, op) { tag ->
        val ndef = Ndef.get(tag) ?: return@run Reply(NOT_NDEF)
        val tech = connect(handle, ndef)
        // Null for a formatted but empty tag.
        val message = (tech as Ndef).ndefMessage
        Reply(OK, message?.toByteArray())
    }

    @JvmStatic
    fun writeNdef(handle: Long, bytes: ByteArray, op: Long) = run(handle, op) { tag ->
        val message = NdefMessage(bytes)
        val ndef = Ndef.get(tag)
        if (ndef != null) {
            val tech = connect(handle, ndef) as Ndef
            if (!tech.isWritable) return@run Reply(READ_ONLY)
            if (bytes.size > tech.maxSize) return@run Reply(TOO_LARGE, extra = tech.maxSize)
            tech.writeNdefMessage(message)
            return@run Reply(OK)
        }
        val formatable = NdefFormatable.get(tag) ?: return@run Reply(NOT_NDEF)
        (connect(handle, formatable) as NdefFormatable).format(message)
        Reply(OK)
    }

    @JvmStatic
    fun transceive(handle: Long, bytes: ByteArray, op: Long) = run(handle, op) { tag ->
        val tech: TagTechnology = IsoDep.get(tag) ?: NfcA.get(tag) ?: NfcB.get(tag)
            ?: NfcF.get(tag) ?: NfcV.get(tag) ?: MifareUltralight.get(tag)
            ?: return@run Reply(NO_RAW_TECH)
        val response = when (val c = connect(handle, tech)) {
            is IsoDep -> c.transceive(bytes)
            is NfcA -> c.transceive(bytes)
            is NfcB -> c.transceive(bytes)
            is NfcF -> c.transceive(bytes)
            is NfcV -> c.transceive(bytes)
            is MifareUltralight -> c.transceive(bytes)
            else -> return@run Reply(NO_RAW_TECH)
        }
        Reply(OK, response)
    }

    private class Reply(
        val status: Int,
        val data: ByteArray? = null,
        val message: String? = null,
        val extra: Int = 0,
    )

    private fun discovered(session: Long, tag: Tag) {
        val handle = synchronized(lock) {
            if (activeSession != session) return
            val h = nextHandle++
            tags[h] = tag
            h
        }
        nativeTagDiscovered(session, handle, tag.id ?: ByteArray(0), tag.techList)
    }

    private fun run(handle: Long, op: Long, body: (Tag) -> Reply) {
        io.execute {
            val reply = try {
                val tag = tags[handle]
                if (tag == null) Reply(TAG_LOST) else body(tag)
            } catch (_: TagLostException) {
                Reply(TAG_LOST)
            } catch (e: FormatException) {
                Reply(ERROR, message = "malformed NDEF: ${e.message}")
            } catch (e: IOException) {
                Reply(ERROR, message = e.message ?: e.toString())
            } catch (e: Throwable) {
                Reply(ERROR, message = e.message ?: e.toString())
            }
            nativeOpResult(op, reply.status, reply.data, reply.message, reply.extra)
        }
    }

    /** The tag's connected [want], connecting it (and closing any other
     *  connected technology) when it is not the current one. Runs on `io`. */
    private fun connect(handle: Long, want: TagTechnology): TagTechnology {
        val current = connected[handle]
        if (current != null && current.javaClass == want.javaClass && current.isConnected) {
            return current
        }
        current?.let(::closeQuietly)
        want.connect()
        connected[handle] = want
        return want
    }

    private fun closeQuietly(tech: TagTechnology) {
        try {
            tech.close()
        } catch (_: IOException) {
        }
    }

    private fun onMain(block: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) block()
        else Handler(Looper.getMainLooper()).post(block)
    }

    @JvmStatic
    private external fun nativeTagDiscovered(
        session: Long,
        handle: Long,
        uid: ByteArray,
        techs: Array<String>,
    )

    @JvmStatic
    private external fun nativeOpResult(
        op: Long,
        status: Int,
        data: ByteArray?,
        message: String?,
        extra: Int,
    )
}
