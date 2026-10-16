//! On-device regression probe for net's Android transport: a request
//! `timeout` must be a deadline on the whole exchange and resolve
//! `Error::Timeout` (the bug: it was ignored, and a server that never
//! answered hung the request forever). Each case prints `PASS` or `FAIL`; `run.sh`
//! fails unless every case passes.
//!
//! The cases mirror `tests/timeout.rs`, which covers the same contract for
//! the reqwest and NSURLSession arms (host and iOS simulator), plus the
//! cancel path the timeout change refactored, plus dropping the request
//! future with no cancel token: that must disconnect the exchange too
//! rather than leave the worker blocked on the connection until the server
//! answers — forever, against a server that never does.

use std::future::Future;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::pin::pin;
use std::sync::{mpsc, Arc};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use jni::objects::JClass;
use jni::sys::jstring;
use jni::JNIEnv;
use net::{cancel_token, Client, Error};

/// How long a case may take before its timeout counts as ignored.
const IGNORED_AFTER: Duration = Duration::from_secs(10);
/// How late a timeout may land and still pass.
const SLACK: Duration = Duration::from_secs(4);

struct Unpark(std::thread::Thread);
impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// net introduces no runtime, so a parking waker is all a test needs.
fn block_on<F: Future>(f: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

#[derive(Clone, Copy)]
enum Server {
    /// Read the request, never answer.
    NeverRespond,
    /// A 200 head promising 100 kB, then one byte every 100 ms.
    TrickleBody,
    /// `200 OK`, body `hi`.
    Prompt,
}

fn serve(behavior: Server) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            std::thread::spawn(move || {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                match behavior {
                    Server::NeverRespond => {
                        let _ = s.read(&mut buf);
                    }
                    Server::TrickleBody => {
                        let head = b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\n";
                        if s.write_all(head).is_err() {
                            return;
                        }
                        while s.write_all(b"x").is_ok() {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                    Server::Prompt => {
                        let reply = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\
                                      Connection: close\r\n\r\nhi";
                        let _ = s.write_all(reply);
                    }
                }
            });
        }
    });
    format!("http://{addr}/")
}

/// Run one request on its own thread and judge the outcome. `expect`
/// checks the result; `within` bounds the elapsed time.
fn case(
    name: &str,
    within: Duration,
    expect: fn(&Result<String, Error>) -> bool,
    request: impl FnOnce() -> Result<String, Error> + Send + 'static,
) -> String {
    let (tx, rx) = mpsc::channel();
    let start = Instant::now();
    std::thread::spawn(move || {
        let _ = tx.send(request());
    });
    match rx.recv_timeout(IGNORED_AFTER) {
        Ok(result) => {
            let elapsed = start.elapsed();
            let verdict = if expect(&result) && elapsed < within {
                "PASS"
            } else {
                "FAIL"
            };
            format!(
                "{verdict} {name}: {result:?} after {} ms",
                elapsed.as_millis()
            )
        }
        Err(_) => format!("FAIL {name}: still pending after {IGNORED_AFTER:?} (timeout ignored)"),
    }
}

/// A server that never answers and reports, per connection, when the
/// request head arrived and when the client closed the socket.
fn serve_watching_close() -> (String, mpsc::Receiver<()>, mpsc::Receiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (received_tx, received) = mpsc::channel();
    let (closed_tx, closed) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { break };
            let received_tx = received_tx.clone();
            let closed_tx = closed_tx.clone();
            std::thread::spawn(move || {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = received_tx.send(());
                loop {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => {
                            let _ = closed_tx.send(());
                            return;
                        }
                        Ok(_) => {}
                    }
                }
            });
        }
    });
    (format!("http://{addr}/"), received, closed)
}

struct NoopWake;
impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

/// Poll a request (no cancel token, no timeout) once — and, with
/// `drop_after_head`, until the server holds its head — then drop the
/// future, and require that no request stays held open on the server.
fn drop_case(name: &str, drop_after_head: bool) -> String {
    let (url, received, closed) = serve_watching_close();
    let mut fut = Box::pin(Client::new().get(url).send());
    let waker = Waker::from(Arc::new(NoopWake));
    let mut cx = Context::from_waker(&waker);
    if let Poll::Ready(r) = fut.as_mut().poll(&mut cx) {
        return format!("FAIL {name}: resolved early: {r:?}");
    }
    if drop_after_head {
        if received.recv_timeout(IGNORED_AFTER).is_err() {
            return format!("FAIL {name}: request never reached the server");
        }
        if let Poll::Ready(r) = fut.as_mut().poll(&mut cx) {
            return format!("FAIL {name}: resolved early: {r:?}");
        }
    }
    let start = Instant::now();
    drop(fut);
    // Either the exchange never reached the server (aborted before it
    // connected), or the server sees the client close it.
    let reached = drop_after_head || received.recv_timeout(Duration::from_secs(2)).is_ok();
    if !reached {
        return format!("PASS {name}: never connected");
    }
    match closed.recv_timeout(SLACK) {
        Ok(()) => format!(
            "PASS {name}: connection closed {} ms after drop",
            start.elapsed().as_millis()
        ),
        Err(_) => format!("FAIL {name}: connection still open {SLACK:?} after drop"),
    }
}

fn get(url: String, timeout: Option<Duration>) -> Result<String, Error> {
    block_on(async move {
        let mut req = Client::new().get(url);
        if let Some(t) = timeout {
            req = req.timeout(t);
        }
        req.send().await?.text().await
    })
}

fn is_timeout(r: &Result<String, Error>) -> bool {
    matches!(r, Err(Error::Timeout))
}

#[no_mangle]
pub extern "system" fn Java_NetProbe_run(env: JNIEnv, _class: JClass) -> jstring {
    let vm = env.get_java_vm().expect("JavaVM");
    // net reads only the JavaVM from ndk-context. app_process has no
    // Android Context, and HttpURLConnection needs none.
    unsafe {
        ndk_context::initialize_android_context(
            vm.get_java_vm_pointer().cast(),
            std::ptr::null_mut(),
        )
    };

    let never = serve(Server::NeverRespond);
    let trickle = serve(Server::TrickleBody);
    let prompt = serve(Server::Prompt);
    let half_second = Duration::from_millis(500);
    let one_second = Duration::from_secs(1);

    let report = [
        case(
            "never-responding server, timeout 500 ms",
            half_second + SLACK,
            is_timeout,
            {
                let url = never.clone();
                move || get(url, Some(half_second))
            },
        ),
        case(
            "trickling body, timeout 1 s (deadline covers the body)",
            one_second + SLACK,
            is_timeout,
            {
                let url = trickle.clone();
                move || get(url, Some(one_second))
            },
        ),
        case(
            "never-responding server, client default timeout 500 ms",
            half_second + SLACK,
            is_timeout,
            {
                let url = never.clone();
                move || {
                    block_on(async move {
                        let client = Client::builder().timeout(half_second).build();
                        client.get(url).send().await?.text().await
                    })
                }
            },
        ),
        case(
            "prompt server, timeout 5 s (does not fire)",
            SLACK,
            |r| matches!(r, Ok(b) if b == "hi"),
            {
                let url = prompt.clone();
                move || get(url, Some(Duration::from_secs(5)))
            },
        ),
        case(
            "never-responding server, cancel at 300 ms, no timeout",
            one_second + SLACK,
            |r| matches!(r, Err(Error::Cancelled)),
            {
                let url = never.clone();
                move || {
                    let (handle, token) = cancel_token();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(300));
                        handle.cancel();
                    });
                    block_on(async move {
                        Client::new()
                            .get(url)
                            .cancel_on(token)
                            .send()
                            .await?
                            .text()
                            .await
                    })
                }
            },
        ),
        drop_case(
            "never-responding server, future dropped mid-flight, no cancel token",
            true,
        ),
        drop_case(
            "future dropped right after the first poll, no cancel token",
            false,
        ),
    ];
    env.new_string(report.join("\n"))
        .expect("report string")
        .into_raw()
}
