//! `net` requests driven by an executor that is NOT tokio.
//!
//! The framework polls futures on its own executor (`spawn_async` /
//! `runtime_shared`'s async driver, a park/unpark loop on a worker thread,
//! the platform event loop) — never inside a tokio runtime. Every test here
//! drives the request with [`block_on`], a bare park/unpark executor with no
//! tokio context on the calling thread, exactly like those callers.
//!
//! The local test servers run on a tokio runtime the TEST owns, on its own
//! worker threads; the client side never enters it.
//!
//! Regression: the Linux/Windows transport (`src/native.rs`) awaited async
//! `reqwest` on the caller's executor. reqwest's DNS + socket I/O need an
//! ambient tokio reactor, so every request from a framework app on the
//! Linux or Windows backend panicked with "there is no reactor running"
//! (and the wgpu engine carried its own private tokio runtime per image
//! fetch to dodge it). On Apple/Android these tests pin that the native
//! arms already need no runtime.

// Native-only: the servers are tokio, which cannot build for wasm32.
#![cfg(not(target_arch = "wasm32"))]

use std::future::Future;
use std::sync::mpsc;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use net::{cancel_token, Client, Error};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Drive `fut` to completion on the calling thread with a park/unpark
/// loop — no tokio runtime is entered, so nothing in the future can lean
/// on an ambient reactor.
fn block_on<F: Future>(fut: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}

/// How the test server treats each request after reading its head.
#[derive(Clone, Copy)]
enum Behavior {
    /// Answer immediately with `200 OK` and the body `hello`.
    Prompt,
    /// Never answer; report on the channel once the client closes the
    /// connection (read returns EOF / error).
    HoldUntilClientCloses,
}

/// A loopback server on its own tokio runtime. Dropping it shuts the
/// runtime down.
struct Server {
    base_url: String,
    /// Fires once per request head the server has read.
    received: mpsc::Receiver<()>,
    /// Fires once per connection the client closed while held.
    closed: mpsc::Receiver<()>,
    _rt: tokio::runtime::Runtime,
}

fn serve(behavior: Behavior) -> Server {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("server runtime");
    let (closed_tx, closed) = mpsc::channel();
    let (received_tx, received) = mpsc::channel();
    // `block_on` only to bind; the accept loop then runs on the runtime's
    // worker thread after this returns, outside the test thread.
    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    rt.spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let closed_tx = closed_tx.clone();
            let received_tx = received_tx.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = received_tx.send(());
                match behavior {
                    Behavior::Prompt => {
                        let reply = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\
                                     Connection: close\r\n\r\nhello";
                        let _ = stream.write_all(reply.as_bytes()).await;
                    }
                    Behavior::HoldUntilClientCloses => loop {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => {
                                let _ = closed_tx.send(());
                                return;
                            }
                            Ok(_) => {}
                        }
                    },
                }
            });
        }
    });
    Server {
        base_url: format!("http://{addr}"),
        received,
        closed,
        _rt: rt,
    }
}

/// Far above every deadline used here; a request still pending this long
/// ignored its cancel / timeout.
const SLACK: Duration = Duration::from_secs(5);

#[test]
fn regression_native_transport_needs_no_ambient_tokio_runtime() {
    let server = serve(Behavior::Prompt);
    let client = Client::new();
    let url = format!("{}/x", server.base_url);
    let result = block_on(async move {
        let resp = client.get(url).send().await?;
        let status = resp.status();
        Ok::<_, Error>((status, resp.text().await?))
    });
    let (status, body) = result.expect("request on a non-tokio executor");
    assert_eq!(status, 200);
    assert_eq!(body, "hello");
}

#[test]
fn regression_one_client_serves_requests_from_many_executor_threads() {
    // The connection pool must not be tied to whichever executor made the
    // first request: two independent non-tokio threads share one Client.
    let server = serve(Behavior::Prompt);
    let client = Client::new();
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let client = client.clone();
            let url = format!("{}/x", server.base_url);
            std::thread::spawn(move || {
                block_on(async move { client.get(url).send().await?.text().await })
            })
        })
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap().expect("request"), "hello");
    }
}

#[test]
fn regression_cancel_mid_flight_on_foreign_executor_returns_cancelled() {
    let server = serve(Behavior::HoldUntilClientCloses);
    let (handle, token) = cancel_token();
    let client = Client::new();
    let request = client.get(format!("{}/slow", server.base_url)).cancel_on(token);

    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        handle.cancel();
    });
    let start = Instant::now();
    let result = block_on(request.send());
    canceller.join().unwrap();

    assert!(
        matches!(result, Err(Error::Cancelled)),
        "expected Error::Cancelled, got {result:?}"
    );
    assert!(start.elapsed() < SLACK, "cancel took {:?}", start.elapsed());
    // On the reqwest arm, cancelling must abort the exchange on the wire
    // too, not leave it running unobserved on the I/O runtime: the server
    // sees the connection close. (Not asserted on Apple: NSURLSession owns
    // its sockets and keeps a cancelled task's connection open — measured
    // still open 70 s after `[task cancel]` on macOS 26.)
    #[cfg(not(any(
        target_os = "ios",
        target_os = "macos",
        target_os = "tvos",
        target_os = "android"
    )))]
    server
        .closed
        .recv_timeout(SLACK)
        .expect("cancel must close the in-flight connection");
    let _ = &server;
}

#[test]
fn regression_timeout_on_foreign_executor_resolves_timeout() {
    let server = serve(Behavior::HoldUntilClientCloses);
    let client = Client::new();
    let request = client
        .get(format!("{}/slow", server.base_url))
        .timeout(Duration::from_millis(300));

    let start = Instant::now();
    let result = block_on(request.send());
    let elapsed = start.elapsed();

    assert!(
        matches!(result, Err(Error::Timeout)),
        "expected Error::Timeout, got {result:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(250) && elapsed < SLACK,
        "timeout landed at {elapsed:?}"
    );
}

/// reqwest arm only: the observable is the server seeing the socket close,
/// which NSURLSession does not do on cancel (see the cancel test above).
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "tvos",
    target_os = "android"
)))]
#[test]
fn regression_dropping_the_request_future_closes_the_connection() {
    // A caller that stops polling and drops the future (a component
    // unmounting, a `select` losing) must abort the request, the same as
    // reqwest's cancel-on-drop when it ran on the caller's executor.
    let server = serve(Behavior::HoldUntilClientCloses);
    let client = Client::new();
    let url = format!("{}/slow", server.base_url);
    let mut fut = Box::pin(client.get(url).send());

    // Poll until the server has the request head, then drop the future.
    let waker = Waker::from(Arc::new(NoopWake));
    let mut cx = Context::from_waker(&waker);
    let deadline = Instant::now() + SLACK;
    loop {
        if let Poll::Ready(r) = fut.as_mut().poll(&mut cx) {
            panic!("held request resolved early: {r:?}");
        }
        if server.received.try_recv().is_ok() {
            break;
        }
        assert!(Instant::now() < deadline, "request never reached the server");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(fut);
    server
        .closed
        .recv_timeout(SLACK)
        .expect("dropping the future must close the in-flight connection");
}

#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "tvos",
    target_os = "android"
)))]
struct NoopWake;
#[cfg(not(any(
    target_os = "ios",
    target_os = "macos",
    target_os = "tvos",
    target_os = "android"
)))]
impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}
