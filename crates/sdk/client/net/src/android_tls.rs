//! `wss://` transport on Android: a TLS socket from the platform's
//! `SSLSocketFactory`, read and written over JNI.
//!
//! The platform stack means the system certificate store, the app's network
//! security config (pinning, user CAs where the app allows them) and no
//! bundled TLS library, the same posture as the HTTP arm's
//! `HttpURLConnection`. Only framework classes are used, so the calls
//! resolve from any thread, the reader thread included.
//!
//! The connection is made the way `HttpsURLConnection` makes one: a plain
//! `Socket` connected with a timeout, layered with
//! `SSLSocketFactory.createSocket(socket, host, port, true)` (which sets
//! SNI), an explicit handshake, then the host-name check. `SSLSocket`
//! itself does not verify the host name; skipping
//! `HttpsURLConnection.getDefaultHostnameVerifier()` would accept any valid
//! certificate for any host.
//!
//! [`TlsInput`] does blocking reads (on `ws_pump`'s reader thread);
//! [`TlsOutput`] writes on the I/O thread and closes the socket when
//! dropped, which ends the blocked read.

use std::io;

use jni::objects::{GlobalRef, JObject, JValue};
use jni::{JNIEnv, JavaVM};

use crate::android::take_pending_exception;
use crate::error::Error;

/// Bound on the TCP connect and on the TLS handshake, each.
const CONNECT_TIMEOUT_MS: i32 = 30_000;

/// A JNI failure, with the pending Java exception's text when there is one.
struct Fail(String);

impl From<jni::errors::Error> for Fail {
    fn from(e: jni::errors::Error) -> Self {
        Fail(format!("JNI: {e}"))
    }
}

/// Attach this thread to the JVM for its whole life. The I/O and reader
/// threads make a JNI call per read or write; a per-call attach would
/// detach between them.
fn env() -> io::Result<JNIEnv<'static>> {
    static VM: std::sync::OnceLock<JavaVM> = std::sync::OnceLock::new();
    let vm = match VM.get() {
        Some(vm) => vm,
        None => {
            let ctx = ndk_context::android_context();
            let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }
                .map_err(|e| io::Error::other(format!("no JavaVM: {e}")))?;
            VM.get_or_init(|| vm)
        }
    };
    vm.attach_current_thread_permanently()
        .map_err(|e| io::Error::other(format!("JNI attach failed: {e}")))
}

/// `r`, or the pending Java exception's text.
fn check<T>(env: &mut JNIEnv<'_>, r: jni::errors::Result<T>) -> Result<T, Fail> {
    r.map_err(|e| Fail(take_pending_exception(env).unwrap_or_else(|| format!("JNI: {e}"))))
}

/// Connect, handshake and verify `host`. Runs on the I/O thread.
pub(crate) fn connect(host: &str, port: u16) -> Result<(TlsInput, TlsOutput), Error> {
    let mut env = env().map_err(|e| Error::Network(e.to_string()))?;
    let result = env.with_local_frame(16, |env| open(env, host, port));
    result.map_err(|Fail(msg)| Error::Network(format!("wss://{host}:{port}: {msg}")))
}

fn open(env: &mut JNIEnv<'_>, host: &str, port: u16) -> Result<(TlsInput, TlsOutput), Fail> {
    let host_j = env.new_string(host)?;
    let addr = env.new_object(
        "java/net/InetSocketAddress",
        "(Ljava/lang/String;I)V",
        &[JValue::Object(&host_j), JValue::Int(port.into())],
    );
    let addr = check(env, addr)?;
    let plain = env.new_object("java/net/Socket", "()V", &[]);
    let plain = check(env, plain)?;
    let r = env.call_method(
        &plain,
        "connect",
        "(Ljava/net/SocketAddress;I)V",
        &[JValue::Object(&addr), JValue::Int(CONNECT_TIMEOUT_MS)],
    );
    check(env, r)?;

    let factory = env.call_static_method(
        "javax/net/ssl/SSLSocketFactory",
        "getDefault",
        "()Ljavax/net/SocketFactory;",
        &[],
    );
    let factory = check(env, factory)?.l()?;
    let ssl = env.call_method(
        &factory,
        "createSocket",
        "(Ljava/net/Socket;Ljava/lang/String;IZ)Ljava/net/Socket;",
        &[
            JValue::Object(&plain),
            JValue::Object(&host_j),
            JValue::Int(port.into()),
            JValue::Bool(1),
        ],
    );
    let ssl = check(env, ssl)?.l()?;
    // From here on, any failure must close the socket.
    match handshake(env, &ssl, &host_j, host) {
        Ok(streams) => Ok(streams),
        Err(e) => {
            let _ = env.call_method(&ssl, "close", "()V", &[]);
            let _ = env.exception_clear();
            Err(e)
        }
    }
}

fn handshake(
    env: &mut JNIEnv<'_>,
    ssl: &JObject<'_>,
    host_j: &JObject<'_>,
    host: &str,
) -> Result<(TlsInput, TlsOutput), Fail> {
    let r = env.call_method(ssl, "setSoTimeout", "(I)V", &[JValue::Int(CONNECT_TIMEOUT_MS)]);
    check(env, r)?;
    let r = env.call_method(ssl, "startHandshake", "()V", &[]);
    check(env, r)?;

    let verifier = env.call_static_method(
        "javax/net/ssl/HttpsURLConnection",
        "getDefaultHostnameVerifier",
        "()Ljavax/net/ssl/HostnameVerifier;",
        &[],
    );
    let verifier = check(env, verifier)?.l()?;
    let session = env.call_method(ssl, "getSession", "()Ljavax/net/ssl/SSLSession;", &[]);
    let session = check(env, session)?.l()?;
    let ok = env.call_method(
        &verifier,
        "verify",
        "(Ljava/lang/String;Ljavax/net/ssl/SSLSession;)Z",
        &[JValue::Object(host_j), JValue::Object(&session)],
    );
    if !check(env, ok)?.z()? {
        return Err(Fail(format!("the server's certificate is not valid for {host}")));
    }

    // Reads block on the pump's reader thread with no timeout; a quiet
    // connection is not an error. Closing the socket ends the read.
    let r = env.call_method(ssl, "setSoTimeout", "(I)V", &[JValue::Int(0)]);
    check(env, r)?;
    let input = env.call_method(ssl, "getInputStream", "()Ljava/io/InputStream;", &[]);
    let input = check(env, input)?.l()?;
    let output = env.call_method(ssl, "getOutputStream", "()Ljava/io/OutputStream;", &[]);
    let output = check(env, output)?.l()?;
    Ok((
        TlsInput {
            stream: env.new_global_ref(input)?,
            buf: None,
        },
        TlsOutput {
            socket: env.new_global_ref(ssl)?,
            stream: env.new_global_ref(output)?,
        },
    ))
}

/// The socket's `InputStream`; blocking reads.
pub(crate) struct TlsInput {
    stream: GlobalRef,
    /// A Java `byte[]` reused across reads, and its length.
    buf: Option<(GlobalRef, i32)>,
}

impl io::Read for TlsInput {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut env = env()?;
        let want = i32::try_from(out.len()).unwrap_or(i32::MAX);
        if self.buf.as_ref().map_or(true, |(_, len)| *len < want) {
            let arr = env
                .new_byte_array(want)
                .and_then(|a| env.new_global_ref(a))
                .map_err(|e| io::Error::other(format!("JNI: {e}")))?;
            self.buf = Some((arr, want));
        }
        let (buf, _) = self.buf.as_ref().expect("allocated above");
        let stream = &self.stream;
        let read = env.with_local_frame(4, |env| -> Result<usize, Fail> {
            let buf: &jni::objects::JByteArray = buf.as_obj().into();
            let r = env.call_method(stream.as_obj(), "read", "([BII)I", &[
                JValue::Object(buf),
                JValue::Int(0),
                JValue::Int(want),
            ]);
            let n = check(env, r)?.i()?;
            if n <= 0 {
                // -1 is end of stream.
                return Ok(0);
            }
            let n = n as usize;
            // SAFETY: `jbyte` is `i8`; reinterpreting `u8` storage as `i8`
            // of the same length is sound.
            let dst = unsafe { &mut *(&mut out[..n] as *mut [u8] as *mut [i8]) };
            env.get_byte_array_region(buf, 0, dst)?;
            Ok(n)
        });
        read.map_err(|Fail(msg)| io::Error::new(io::ErrorKind::ConnectionAborted, msg))
    }
}

/// The socket's `OutputStream`; closes the socket on drop.
pub(crate) struct TlsOutput {
    socket: GlobalRef,
    stream: GlobalRef,
}

impl io::Write for TlsOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut env = env()?;
        let written = env.with_local_frame(4, |env| -> Result<(), Fail> {
            let arr = env.byte_array_from_slice(buf)?;
            let len = i32::try_from(buf.len()).map_err(|_| Fail("write too large".into()))?;
            let r = env.call_method(self.stream.as_obj(), "write", "([BII)V", &[
                JValue::Object(&arr),
                JValue::Int(0),
                JValue::Int(len),
            ]);
            check(env, r)?;
            Ok(())
        });
        written
            .map(|()| buf.len())
            .map_err(|Fail(msg)| io::Error::new(io::ErrorKind::BrokenPipe, msg))
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut env = env()?;
        let r = env.call_method(self.stream.as_obj(), "flush", "()V", &[]);
        check(&mut env, r)
            .map(drop)
            .map_err(|Fail(msg)| io::Error::new(io::ErrorKind::BrokenPipe, msg))
    }
}

impl Drop for TlsOutput {
    fn drop(&mut self) {
        if let Ok(mut env) = env() {
            let _ = env.call_method(self.socket.as_obj(), "close", "()V", &[]);
            // Never leave an exception pending on an attached thread.
            let _ = env.exception_clear();
        }
    }
}
