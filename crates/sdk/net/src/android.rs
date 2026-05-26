//! Android transport, built on `java.net.HttpURLConnection` via JNI.
//!
//! Picked HttpURLConnection over OkHttp so the SDK has zero
//! Gradle/JAR footprint — see `crates/sdk/net/Cargo.toml` header
//! for the rationale.
//!
//! Threading: HttpURLConnection's `getResponseCode` and stream reads
//! block; they MUST run off the main looper. Each request spawns a
//! short-lived worker thread that attaches to the JavaVM via
//! `ndk_context`, does its JNI work, and sends the result back over
//! a `futures-channel` oneshot.
//!
//! Cancellation: as soon as the worker has the connection, it
//! promotes it to a JNI `GlobalRef` and parks the ref in a shared
//! `Arc<Mutex<Option<GlobalRef>>>`. When the awaiter's cancel token
//! fires, the watcher takes that ref out and — on a fresh JVM-
//! attached thread — calls `conn.disconnect()`. That causes the
//! worker's blocking `getResponseCode()` / `read()` to throw an
//! IOException, which we map to `Error::Network`; the worker's
//! result is then discarded because the awaiter has already
//! returned `Err(Error::Cancelled)`.

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use futures_channel::oneshot;
use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::JavaVM;

use crate::cancel::CancelToken;
use crate::error::Error;
use crate::headers::Headers;
use crate::method::Method;
use crate::response::Response;

pub(crate) struct Transport;

impl Transport {
    pub(crate) fn new() -> Self {
        Self
    }
}

/// Shared slot the worker thread parks the connection's `GlobalRef`
/// into and the cancel watcher takes it out of. `Option` because
/// either side might claim it first:
///
/// - Worker on natural completion: `take()` returns Some, drops the
///   GlobalRef (which calls `DeleteGlobalRef` on the JVM).
/// - Cancel watcher on token fire: `take()` returns Some, uses the
///   ref to call `disconnect()` on a fresh thread, then drops it.
/// - Whoever loses the race gets `None` and no-ops.
type ConnSlot = Arc<Mutex<Option<GlobalRef>>>;

pub(crate) async fn send(
    _transport: &Transport,
    method: Method,
    url: String,
    headers: Headers,
    body: Vec<u8>,
    _timeout: Option<Duration>,
    cancel: Option<CancelToken>,
) -> Result<Response, Error> {
    let (tx, rx) = oneshot::channel::<Result<Response, Error>>();

    // SAFETY: `ndk_context::android_context()` is the documented entry
    // point for getting the host's `JavaVM` pointer. The host
    // installs it once at startup; calling before init panics with a
    // clear message, which is the correct failure mode (it's a
    // bootstrap bug, not a network failure).
    let ctx = ndk_context::android_context();
    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
    let vm = unsafe { JavaVM::from_raw(vm_ptr) }
        .map_err(|e| Error::Other(format!("invalid JavaVM pointer: {e}")))?;

    // Slot shared between worker (writes the GlobalRef in, takes it
    // out on completion) and the cancel watcher (takes it out on
    // cancel, uses it to disconnect).
    let conn_slot: ConnSlot = Arc::new(Mutex::new(None));
    let slot_for_worker = conn_slot.clone();

    std::thread::spawn(move || {
        let result = do_request(&vm, method, url, headers, body, slot_for_worker);
        let _ = tx.send(result);
    });

    let receive_future = async move {
        rx.await
            .unwrap_or_else(|_| Err(Error::Other("Android worker thread dropped".into())))
    };

    match cancel {
        None => receive_future.await,
        Some(token) => race_with_cancel(receive_future, token, conn_slot).await,
    }
}

/// RAII helper: on drop, atomically takes any `GlobalRef` left in
/// `conn_slot` and drops it. Declared *after* the `AttachGuard` in
/// `do_request` so it runs first (Rust drops in reverse declaration
/// order) — that puts the `DeleteGlobalRef` call on the JNI-attached
/// worker thread, where it's a single FFI hop rather than a
/// permanent thread attach.
struct SlotGuard<'a>(&'a ConnSlot);
impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        let _ = self.0.lock().unwrap().take();
    }
}

/// Runs on the worker thread. Attaches to the JavaVM, drives the
/// HttpURLConnection through its lifecycle, reads the response,
/// then detaches when the AttachGuard drops.
fn do_request(
    vm: &JavaVM,
    method: Method,
    url: String,
    headers: Headers,
    body: Vec<u8>,
    conn_slot: ConnSlot,
) -> Result<Response, Error> {
    let mut env = vm
        .attach_current_thread()
        .map_err(|e| Error::Other(format!("JNI attach failed: {e}")))?;

    // The guard sits below `env` in declaration order so it drops
    // FIRST. That ensures any leftover `GlobalRef` in the slot
    // is freed via `DeleteGlobalRef` on this attached thread, not
    // on whichever async-runtime thread happens to hold the last
    // `Arc` clone.
    let _slot_guard = SlotGuard(&conn_slot);

    // -----------------------------------------------------------------
    // URL url = new URL(url_str);
    // -----------------------------------------------------------------
    let url_class = env
        .find_class("java/net/URL")
        .map_err(map_jni_err)?;
    let url_str: JString = env.new_string(&url).map_err(map_jni_err)?;
    let url_obj = env
        .new_object(
            url_class,
            "(Ljava/lang/String;)V",
            &[JValue::Object(&JObject::from(url_str))],
        )
        .map_err(map_jni_err)?;

    // -----------------------------------------------------------------
    // HttpURLConnection conn = (HttpURLConnection) url.openConnection();
    // -----------------------------------------------------------------
    let conn_obj = env
        .call_method(&url_obj, "openConnection", "()Ljava/net/URLConnection;", &[])
        .map_err(map_jni_err)?
        .l()
        .map_err(map_jni_err)?;

    // -----------------------------------------------------------------
    // Promote the connection to a JNI global ref and publish it to the
    // shared slot. From this point on, a concurrent cancel can call
    // `disconnect()` on the same Java object from a different thread.
    // The worker continues using `conn_obj` (the local ref); the
    // global ref's purpose is purely to give the cancel watcher a
    // cross-thread handle.
    // -----------------------------------------------------------------
    {
        let global = env.new_global_ref(&conn_obj).map_err(map_jni_err)?;
        *conn_slot.lock().unwrap() = Some(global);
    }

    // -----------------------------------------------------------------
    // conn.setRequestMethod(method);
    // -----------------------------------------------------------------
    let method_str: JString = env.new_string(method.as_str()).map_err(map_jni_err)?;
    env.call_method(
        &conn_obj,
        "setRequestMethod",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&JObject::from(method_str))],
    )
    .map_err(map_jni_err)?;

    // -----------------------------------------------------------------
    // for (k, v) in headers: conn.setRequestProperty(k, v);
    // -----------------------------------------------------------------
    for (name, value) in headers.iter() {
        let n: JString = env.new_string(name).map_err(map_jni_err)?;
        let v: JString = env.new_string(value).map_err(map_jni_err)?;
        env.call_method(
            &conn_obj,
            "setRequestProperty",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[
                JValue::Object(&JObject::from(n)),
                JValue::Object(&JObject::from(v)),
            ],
        )
        .map_err(map_jni_err)?;
    }

    // -----------------------------------------------------------------
    // If body: conn.setDoOutput(true); write to OutputStream; close.
    // -----------------------------------------------------------------
    if !body.is_empty() {
        env.call_method(&conn_obj, "setDoOutput", "(Z)V", &[JValue::Bool(1)])
            .map_err(map_jni_err)?;
        let os = env
            .call_method(&conn_obj, "getOutputStream", "()Ljava/io/OutputStream;", &[])
            .map_err(map_jni_err)?
            .l()
            .map_err(map_jni_err)?;
        let body_arr = env
            .byte_array_from_slice(&body)
            .map_err(map_jni_err)?;
        env.call_method(
            &os,
            "write",
            "([B)V",
            &[JValue::Object(&JObject::from(body_arr))],
        )
        .map_err(map_jni_err)?;
        env.call_method(&os, "close", "()V", &[])
            .map_err(map_jni_err)?;
    }

    // -----------------------------------------------------------------
    // int code = conn.getResponseCode();
    // -----------------------------------------------------------------
    let code = env
        .call_method(&conn_obj, "getResponseCode", "()I", &[])
        .map_err(map_jni_err)?
        .i()
        .map_err(map_jni_err)?;
    let status: u16 = if (0..=u16::MAX as i32).contains(&code) {
        code as u16
    } else {
        0
    };

    // -----------------------------------------------------------------
    // Response headers via getHeaderFields(): Map<String, List<String>>.
    // We flatten each entry's values into our `Headers` map.
    // -----------------------------------------------------------------
    let mut out_headers = Headers::new();
    collect_headers(&mut env, &conn_obj, &mut out_headers).ok();

    // -----------------------------------------------------------------
    // Body: 4xx/5xx use getErrorStream; otherwise getInputStream.
    // -----------------------------------------------------------------
    let stream_method = if status >= 400 {
        "getErrorStream"
    } else {
        "getInputStream"
    };
    let stream_result = env.call_method(
        &conn_obj,
        stream_method,
        "()Ljava/io/InputStream;",
        &[],
    );
    let body_bytes = match stream_result {
        Ok(jv) => {
            let stream = jv.l().map_err(map_jni_err)?;
            if stream.is_null() {
                Vec::new()
            } else {
                read_input_stream(&mut env, &stream).unwrap_or_default()
            }
        }
        Err(_) => Vec::new(),
    };

    // Always disconnect to free the socket promptly. Safe to call even
    // after successful reads; HttpURLConnection's docs encourage it
    // for short-lived requests. The shared-slot cleanup happens via
    // `SlotGuard::drop`, which runs after this function returns.
    let _ = env.call_method(&conn_obj, "disconnect", "()V", &[]);

    Ok(Response {
        status,
        headers: out_headers,
        body: body_bytes,
    })
}

/// Iterate `Map<String, List<String>>` from `getHeaderFields()` and
/// push every (name, value) pair into `out`. Each call adds one
/// entry per value, mirroring HTTP semantics for repeating headers
/// like `Set-Cookie`.
fn collect_headers(
    env: &mut jni::JNIEnv<'_>,
    conn: &JObject<'_>,
    out: &mut Headers,
) -> Result<(), Error> {
    let map = env
        .call_method(conn, "getHeaderFields", "()Ljava/util/Map;", &[])
        .map_err(map_jni_err)?
        .l()
        .map_err(map_jni_err)?;
    if map.is_null() {
        return Ok(());
    }

    let entry_set = env
        .call_method(&map, "entrySet", "()Ljava/util/Set;", &[])
        .map_err(map_jni_err)?
        .l()
        .map_err(map_jni_err)?;
    let iter = env
        .call_method(&entry_set, "iterator", "()Ljava/util/Iterator;", &[])
        .map_err(map_jni_err)?
        .l()
        .map_err(map_jni_err)?;

    loop {
        let has_next = env
            .call_method(&iter, "hasNext", "()Z", &[])
            .map_err(map_jni_err)?
            .z()
            .map_err(map_jni_err)?;
        if !has_next {
            break;
        }
        let entry = env
            .call_method(&iter, "next", "()Ljava/lang/Object;", &[])
            .map_err(map_jni_err)?
            .l()
            .map_err(map_jni_err)?;

        // Map.Entry.getKey() — may be null for HttpURLConnection's
        // status-line pseudo-header (it's keyed as null at index 0).
        let key_obj = env
            .call_method(&entry, "getKey", "()Ljava/lang/Object;", &[])
            .map_err(map_jni_err)?
            .l()
            .map_err(map_jni_err)?;
        if key_obj.is_null() {
            continue;
        }
        let key_str: JString = key_obj.into();
        let key: String = env
            .get_string(&key_str)
            .map_err(map_jni_err)?
            .into();

        // Map.Entry.getValue() — List<String>.
        let values_list = env
            .call_method(&entry, "getValue", "()Ljava/lang/Object;", &[])
            .map_err(map_jni_err)?
            .l()
            .map_err(map_jni_err)?;
        let size = env
            .call_method(&values_list, "size", "()I", &[])
            .map_err(map_jni_err)?
            .i()
            .map_err(map_jni_err)?;
        for i in 0..size {
            let value_obj = env
                .call_method(&values_list, "get", "(I)Ljava/lang/Object;", &[JValue::Int(i)])
                .map_err(map_jni_err)?
                .l()
                .map_err(map_jni_err)?;
            if value_obj.is_null() {
                continue;
            }
            let value_str: JString = value_obj.into();
            let value: String = env
                .get_string(&value_str)
                .map_err(map_jni_err)?
                .into();
            // Clone the key per push — `Headers::append` takes
            // owned strings, and HTTP allows the same header to
            // repeat (e.g. `Set-Cookie`).
            out.append(key.clone(), value);
        }
        let _ = key;
    }
    Ok(())
}

/// Drain a `java.io.InputStream` into a `Vec<u8>`. Uses
/// `read(byte[])` in 4 KiB chunks; bigger buffers don't help
/// noticeably on Android and waste memory for small bodies.
fn read_input_stream(
    env: &mut jni::JNIEnv<'_>,
    stream: &JObject<'_>,
) -> Result<Vec<u8>, Error> {
    const CHUNK: usize = 4096;
    let buf_java = env.new_byte_array(CHUNK as i32).map_err(map_jni_err)?;
    let mut out = Vec::new();
    loop {
        // `JPrimitiveArray<i8>` (what `new_byte_array` returns)
        // derefs to `JObject` via `AsRef`, which is how we pass it
        // as a method argument without consuming the wrapper.
        let read = env
            .call_method(
                stream,
                "read",
                "([B)I",
                &[JValue::Object(buf_java.as_ref())],
            )
            .map_err(map_jni_err)?
            .i()
            .map_err(map_jni_err)?;
        if read < 0 {
            break; // EOF
        }
        if read == 0 {
            continue;
        }
        // Copy the populated prefix of the Java byte[] into Rust.
        let mut chunk = vec![0i8; read as usize];
        env.get_byte_array_region(&buf_java, 0, &mut chunk)
            .map_err(map_jni_err)?;
        out.extend(chunk.into_iter().map(|b| b as u8));
    }
    let _ = env.call_method(stream, "close", "()V", &[]);
    Ok(out)
}

fn map_jni_err(e: jni::errors::Error) -> Error {
    Error::Network(format!("JNI: {e}"))
}

/// Race the worker's `oneshot` against the cancel token. If cancel
/// wins, take the connection's `GlobalRef` out of the shared slot
/// (atomically — the worker will then see `None` on its own
/// cleanup) and spawn a fresh thread that attaches to the JVM and
/// calls `conn.disconnect()`. That terminates the worker's
/// blocking I/O; the worker's `Result` is discarded since the
/// awaiter has already returned `Err(Error::Cancelled)`.
async fn race_with_cancel<F>(
    receive_future: F,
    token: CancelToken,
    conn_slot: ConnSlot,
) -> Result<Response, Error>
where
    F: Future<Output = Result<Response, Error>>,
{
    let mut fut = Box::pin(receive_future);
    let mut cancel_fut = Box::pin(token.cancelled());
    poll_fn(|cx| {
        if let Poll::Ready(()) = Pin::new(&mut cancel_fut).poll(cx) {
            // Atomic take: only one party (this watcher or the
            // worker's SlotGuard) sees `Some`. If the worker has
            // already finished, this is `None` and we skip the
            // disconnect. Either outcome leaves `Err(Cancelled)` to
            // the caller, which is what the cancel contract
            // requires.
            if let Some(global) = conn_slot.lock().unwrap().take() {
                // Spawn a one-shot thread to perform the disconnect.
                // `attach_current_thread` blocks; we can't call it
                // from inside `poll_fn` without holding up the
                // entire async runtime, so the disconnect happens
                // off-thread and we resolve the cancel here. By the
                // time the disconnect lands, the worker's blocking
                // JNI call (likely `getResponseCode` or `read`) has
                // already thrown an IOException and the worker has
                // exited, dropping its `oneshot::Sender`. The
                // disconnect on the worker-side socket is then a
                // no-op; we still need it for the case where the
                // worker is parked inside that blocking call.
                std::thread::spawn(move || {
                    let ctx = ndk_context::android_context();
                    let vm_ptr = ctx.vm() as *mut jni::sys::JavaVM;
                    let vm = match unsafe { JavaVM::from_raw(vm_ptr) } {
                        Ok(vm) => vm,
                        Err(_) => return,
                    };
                    let mut env = match vm.attach_current_thread() {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                    let _ = env.call_method(global.as_obj(), "disconnect", "()V", &[]);
                    // `global` drops here; the AttachGuard is still
                    // alive, so `DeleteGlobalRef` is one FFI hop.
                });
            }
            return Poll::Ready(Err(Error::Cancelled));
        }
        if let Poll::Ready(result) = Pin::new(&mut fut).poll(cx) {
            return Poll::Ready(result);
        }
        Poll::Pending
    })
    .await
}
