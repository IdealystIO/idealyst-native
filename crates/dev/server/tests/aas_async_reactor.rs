//! E2 regression: AAS session-thread async work runs inside a Tokio reactor.
//!
//! Field bug: an app-initiated async task on an AAS session thread (a
//! server-fn that lowers to reqwest → `tokio::net::TcpStream`) panicked
//!
//! ```text
//! thread 'aas-session-…' panicked at tokio/net/tcp/stream.rs:164:
//!   there is no reactor running, must be called from the context of a
//!   Tokio 1.x runtime
//! ```
//!
//! because the session thread drove the future with `pollster::block_on`
//! on a thread that had no Tokio runtime context. After the fix,
//! `dev_server::async_executor::install()` (called per session thread in
//! `run_session_thread`) routes `runtime_core::driver::spawn_async`
//! through a current-thread Tokio runtime, so the socket finds its
//! reactor.
//!
//! These tests don't pull reqwest in; a bare `tokio::net::TcpStream`
//! exercises the *exact* line that panicked (`tcp/stream.rs`), which is
//! the reactor-registration point reqwest sits on top of.

use std::cell::Cell;
use std::net::TcpListener as StdListener;
use std::rc::Rc;

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

/// Spin up a one-shot loopback TCP server on a background thread that
/// accepts a single connection and writes one byte. Returns the bound
/// address so the client future can connect to it. Std sockets (no
/// tokio) so the listener never competes for the reactor under test.
fn one_shot_server() -> std::net::SocketAddr {
    let listener = StdListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let addr = listener.local_addr().expect("listener addr");
    std::thread::spawn(move || {
        if let Ok((mut conn, _)) = listener.accept() {
            use std::io::Write;
            let _ = conn.write_all(&[0x42]);
            let _ = conn.flush();
        }
    });
    addr
}

/// THE regression test. Replicates the session-thread setup: a bare
/// `std::thread` (no ambient runtime), the executor installed exactly as
/// `run_session_thread` does, then app-style async work via
/// `spawn_async`. Before the fix this future panics at
/// `tokio/net/tcp/stream.rs` ("no reactor running"); after it, the
/// connect + read completes and we observe the byte.
#[test]
fn regression_aas_session_async_has_reactor() {
    let addr = one_shot_server();

    // Mirror `run_session_thread`: a fresh worker thread with no Tokio
    // context, install the sidecar executor, then drive author async.
    let handle = std::thread::Builder::new()
        .name("aas-session-test_00000001".into())
        .spawn(move || {
            dev_server::async_executor::install();

            // `!Send` accumulator captured by the future — proves the
            // executor accepts the same non-`Send` futures `spawn_async`
            // promises (Rc/Cell, not Arc/Mutex).
            let got = Rc::new(Cell::new(0u8));
            let got_in = got.clone();

            runtime_core::driver::spawn_async(async move {
                // This is the call that panicked pre-fix: constructing a
                // tokio `TcpStream` registers it with the current
                // runtime's I/O reactor.
                let mut stream = TcpStream::connect(addr)
                    .await
                    .expect("connect to loopback one-shot server");
                let mut buf = [0u8; 1];
                stream
                    .read_exact(&mut buf)
                    .await
                    .expect("read one byte from server");
                got_in.set(buf[0]);
            });

            // `spawn_async`'s native contract drives the future to
            // completion before returning, so the byte is already set.
            got.get()
        })
        .expect("spawn session-like thread");

    let byte = handle.join().expect("session thread must not panic");
    assert_eq!(byte, 0x42, "server byte should have round-tripped");
}

/// Pins the *cause*: the same future, polled with the pre-fix mechanism
/// (`pollster::block_on` on a reactor-less thread), panics with the
/// reactor message. Guards against a future refactor that silently drops
/// the executor install and falls back to pollster — which would
/// reintroduce E2. Connect to a real (std) listener so the failure is
/// the reactor, not a refused connection.
#[test]
fn pollster_without_reactor_panics_on_socket() {
    let addr = one_shot_server();

    let result = std::panic::catch_unwind(|| {
        pollster::block_on(async move {
            let _stream = TcpStream::connect(addr).await;
        });
    });

    let err = result.expect_err("a reactor-less pollster poll of a tokio socket must panic");
    let msg = err
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| err.downcast_ref::<&'static str>().copied())
        .unwrap_or("");
    assert!(
        msg.contains("reactor") || msg.contains("Tokio"),
        "panic should be the missing-reactor error, got: {msg:?}"
    );
}
