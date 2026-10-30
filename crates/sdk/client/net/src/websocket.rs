//! Cross-platform WebSocket client — sibling to the HTTP [`Client`](crate::Client).
//!
//! One async surface (`connect` / `send` / `recv` / `close`, plus
//! close-on-drop) that maps to the platform-native socket on each target,
//! exactly like the HTTP client maps to fetch / NSURLSession /
//! HttpURLConnection / reqwest:
//!
//! | target                         | backend                              |
//! |--------------------------------|--------------------------------------|
//! | web (wasm32)                   | the browser `WebSocket` (web-glue)   |
//! | iOS / macOS / desktop / terminal | sync `tungstenite` on an I/O thread, TLS by rustls |
//! | Android                        | sync `tungstenite` on an I/O thread, TLS by the platform's `SSLSocket` |
//!
//! Two arms ship: web (the browser `WebSocket`) and a shared native arm used by
//! every non-wasm target. iOS/Android reuse the native arm because they're
//! native Rust targets with TCP sockets. On Android, `wss://` runs
//! tungstenite over a TLS socket from the platform's `SSLSocketFactory`
//! (`android_tls.rs`): the system TLS stack and certificate store, the
//! app's network security config, and no bundled TLS library. The OS proxy
//! is not consulted on any native target.
//!
//! # Execution model
//!
//! Per the framework's runtime invariant, this introduces **no async
//! runtime**. On native, a single blocking worker thread owns the socket
//! and does the reads/writes; inbound messages are bridged to the async
//! `recv()` through a `futures-channel` whose cross-thread waker re-polls
//! under the framework's scheduler. On web the browser's event loop is the
//! runtime and callbacks marshal in the same way. Nothing here spins up
//! tokio.

use crate::error::Error;

/// A WebSocket message. Control frames (ping/pong/close) are handled by
/// the transport and never surface here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsMessage {
    /// A UTF-8 text frame.
    Text(String),
    /// A binary frame.
    Binary(Vec<u8>),
}

/// How a WebSocket connection ended — the close code and reason, as
/// RFC 6455 defines them, reported identically on every target.
///
/// The two codes that never travel on the wire are synthesized the way
/// browsers synthesize them, so an app reads the same number everywhere:
/// [`WsClose::NO_STATUS`] (1005) when a close frame carried no code (which
/// is also what a locally-initiated [`WebSocket::close`] ends with), and
/// [`WsClose::ABNORMAL`] (1006) when the connection dropped with no close
/// frame at all — network loss, a killed server, a transport error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsClose {
    /// The close code (`1000` = normal closure; `4000`–`4999` are
    /// application-defined).
    pub code: u16,
    /// The peer's close reason, or empty.
    pub reason: String,
}

impl WsClose {
    /// 1000 — the purpose of the connection was fulfilled.
    pub const NORMAL: u16 = 1000;
    /// 1005 — a close frame arrived without a status code (never sent on
    /// the wire; synthesized).
    pub const NO_STATUS: u16 = 1005;
    /// 1006 — the connection ended without any close frame (never sent on
    /// the wire; synthesized).
    pub const ABNORMAL: u16 = 1006;

    /// Did the connection end with a normal (1000) closure?
    pub fn is_normal(&self) -> bool {
        self.code == Self::NORMAL
    }

    pub(crate) fn new(code: u16, reason: impl Into<String>) -> Self {
        WsClose {
            code,
            reason: reason.into(),
        }
    }
}

/// A connected WebSocket. The connection is closed when this is dropped
/// (so a `use_socket`-style hook gets teardown for free by tying the
/// handle's lifetime to a component scope).
pub struct WebSocket {
    inner: imp::WebSocketImpl,
}

impl WebSocket {
    /// Open a connection to `url` and resolve once the handshake
    /// completes.
    ///
    /// `ws://` and `wss://` work on every target. `wss://` verifies the
    /// server's certificate and host name against the platform's trust
    /// store on Android (including the app's network security config), and
    /// against the bundled Mozilla roots elsewhere on native; a failure is
    /// [`Error::Network`].
    pub async fn connect(url: &str) -> Result<WebSocket, Error> {
        Ok(WebSocket {
            inner: imp::connect(url).await?,
        })
    }

    /// Queue a message for sending. Returns immediately — the actual
    /// write happens on the transport's I/O source. Errors only if the
    /// connection is already closed.
    pub fn send(&self, msg: WsMessage) -> Result<(), Error> {
        self.inner.send(msg)
    }

    /// Await the next inbound message. `None` means the connection closed
    /// (cleanly or otherwise); `Some(Err(_))` is a transport error.
    pub async fn recv(&mut self) -> Option<Result<WsMessage, Error>> {
        self.inner.recv().await
    }

    /// Close the connection. Idempotent; also runs on drop.
    pub fn close(&self) {
        self.inner.close();
    }

    /// How the connection ended, once it has: `None` while it is open,
    /// then the [`WsClose`] code and reason. Set before [`recv`](Self::recv)
    /// yields its final `None`, so the natural read is right after the
    /// receive loop ends:
    ///
    /// ```ignore
    /// while let Some(msg) = ws.recv().await { /* … */ }
    /// match ws.close_status() {
    ///     Some(c) if c.is_normal() => {}                 // server finished
    ///     Some(c) => report(format!("closed ({})", c.code)),
    ///     None => {}
    /// }
    /// ```
    pub fn close_status(&self) -> Option<WsClose> {
        self.inner.close_status()
    }

    /// A cheap, cloneable send handle. Lets one task own the socket for
    /// `recv` (which needs `&mut self`) while other holders `send`
    /// concurrently — the basis for a split / `use_socket` hook.
    pub fn sender(&self) -> WsSender {
        WsSender {
            inner: self.inner.sender(),
        }
    }
}

/// A cloneable send half of a [`WebSocket`]. Sending is independent of
/// the receive loop, so it can be held by a UI scope while the socket is
/// driven elsewhere.
#[derive(Clone)]
pub struct WsSender {
    inner: imp::WsSenderImpl,
}

impl WsSender {
    /// Queue a message for sending. Errors only if the connection closed.
    pub fn send(&self, msg: WsMessage) -> Result<(), Error> {
        self.inner.send(msg)
    }

    /// Close the connection.
    pub fn close(&self) {
        self.inner.close();
    }
}

// ---------------------------------------------------------------------------
// Native arm: sync tungstenite on a blocking I/O worker thread. Used on
// every native target (desktop + iOS + Android) — all have TCP sockets
// and threads.
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::{WsClose, WsMessage};
    use crate::error::Error;

    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc as std_mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures_channel::mpsc as fut_mpsc;
    use futures_channel::oneshot;
    use futures_util::StreamExt;
    use tungstenite::stream::MaybeTlsStream;
    use tungstenite::Message;

    /// Idle poll cadence of the I/O loop: bounds both inbound latency and
    /// outbound flush latency. Small enough to feel instant, large enough
    /// not to busy-spin. (A future mio-based readiness loop could remove
    /// the poll entirely.)
    const POLL_INTERVAL: Duration = Duration::from_millis(2);

    enum Outbound {
        Msg(WsMessage),
        Close,
    }

    pub struct WebSocketImpl {
        outbound: std_mpsc::Sender<Outbound>,
        inbound: fut_mpsc::UnboundedReceiver<Result<WsMessage, Error>>,
        closed: Arc<AtomicBool>,
        close_status: CloseSlot,
    }

    /// How the connection ended, written once by the I/O thread before it
    /// drops the inbound sender (so it is visible by the time `recv`
    /// yields `None`). First write wins: the peer's close frame is the
    /// answer even if a transport error follows it.
    #[derive(Clone, Default)]
    struct CloseSlot(Arc<Mutex<Option<WsClose>>>);

    impl CloseSlot {
        fn record(&self, close: WsClose) {
            let mut slot = self.0.lock().unwrap_or_else(|p| p.into_inner());
            if slot.is_none() {
                *slot = Some(close);
            }
        }
        fn get(&self) -> Option<WsClose> {
            self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
        }
    }

    pub async fn connect(url: &str) -> Result<WebSocketImpl, Error> {
        let (out_tx, out_rx) = std_mpsc::channel::<Outbound>();
        let (in_tx, in_rx) = fut_mpsc::unbounded::<Result<WsMessage, Error>>();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), Error>>();
        let closed = Arc::new(AtomicBool::new(false));
        let close_status = CloseSlot::default();

        let url = url.to_string();
        let closed_thread = closed.clone();
        let status_thread = close_status.clone();
        std::thread::Builder::new()
            .name("net-ws".into())
            .spawn(move || io_loop(url, out_rx, in_tx, ready_tx, closed_thread, status_thread))
            .map_err(|e| Error::Other(format!("ws thread spawn failed: {e}")))?;

        // The handshake runs on the worker thread; await its result so
        // `connect` only resolves once the socket is live.
        match ready_rx.await {
            Ok(Ok(())) => Ok(WebSocketImpl {
                outbound: out_tx,
                inbound: in_rx,
                closed,
                close_status,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Error::Network("ws worker dropped during handshake".into())),
        }
    }

    impl WebSocketImpl {
        pub fn send(&self, msg: WsMessage) -> Result<(), Error> {
            self.outbound
                .send(Outbound::Msg(msg))
                .map_err(|_| Error::Network("websocket is closed".into()))
        }

        pub async fn recv(&mut self) -> Option<Result<WsMessage, Error>> {
            self.inbound.next().await
        }

        pub fn close(&self) {
            self.closed.store(true, Ordering::Relaxed);
            let _ = self.outbound.send(Outbound::Close);
        }

        pub fn close_status(&self) -> Option<WsClose> {
            self.close_status.get()
        }

        pub fn sender(&self) -> WsSenderImpl {
            WsSenderImpl {
                outbound: self.outbound.clone(),
                closed: self.closed.clone(),
            }
        }
    }

    /// Cloneable send handle: the outbound channel + the shared closed
    /// flag. `std_mpsc::Sender` is `Clone`, so many senders feed the one
    /// I/O thread; it stops when the last sender drops or `closed` is set.
    #[derive(Clone)]
    pub struct WsSenderImpl {
        outbound: std_mpsc::Sender<Outbound>,
        closed: Arc<AtomicBool>,
    }

    impl WsSenderImpl {
        pub fn send(&self, msg: WsMessage) -> Result<(), Error> {
            self.outbound
                .send(Outbound::Msg(msg))
                .map_err(|_| Error::Network("websocket is closed".into()))
        }

        pub fn close(&self) {
            self.closed.store(true, Ordering::Relaxed);
            let _ = self.outbound.send(Outbound::Close);
        }
    }

    impl Drop for WebSocketImpl {
        fn drop(&mut self) {
            // Signal the worker to close; dropping `outbound` also
            // disconnects the channel as a backstop.
            self.closed.store(true, Ordering::Relaxed);
        }
    }

    fn io_loop(
        url: String,
        out_rx: std_mpsc::Receiver<Outbound>,
        in_tx: fut_mpsc::UnboundedSender<Result<WsMessage, Error>>,
        ready_tx: oneshot::Sender<Result<(), Error>>,
        closed: Arc<AtomicBool>,
        status: CloseSlot,
    ) {
        #[cfg(target_os = "android")]
        if url.starts_with("wss://") {
            match connect_platform_tls(&url) {
                Ok(socket) => run(socket, out_rx, in_tx, ready_tx, closed, status),
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
            return;
        }
        // Blocking handshake (TLS by rustls for `wss://`, except on Android,
        // handled above).
        let mut socket = match tungstenite::connect(&url) {
            Ok((socket, _resp)) => socket,
            Err(e) => {
                let _ = ready_tx.send(Err(map_err(e)));
                return;
            }
        };
        // tungstenite supports a non-blocking underlying stream: `read()`
        // returns `WouldBlock` when no full message is buffered yet, and
        // partial frames are retained internally across calls — so we can
        // interleave reads and writes on one thread without a read timeout
        // corrupting framing.
        if let Err(e) = set_nonblocking(&mut socket) {
            let _ = ready_tx.send(Err(Error::Network(format!("set_nonblocking: {e}"))));
            return;
        }
        run(socket, out_rx, in_tx, ready_tx, closed, status);
    }

    /// `wss://` on Android: TLS from the platform (`android_tls`), with a
    /// reader thread making the socket's reads non-blocking (`ws_pump`).
    #[cfg(target_os = "android")]
    fn connect_platform_tls(
        url: &str,
    ) -> Result<tungstenite::WebSocket<crate::ws_pump::PumpedStream<crate::android_tls::TlsOutput>>, Error>
    {
        let (host, port) = host_port(url)?;
        let (input, output) = crate::android_tls::connect(&host, port)?;
        let stream = crate::ws_pump::pumped(input, output)
            .map_err(|e| Error::Other(format!("ws reader thread spawn failed: {e}")))?;
        client_handshake(url, stream)
    }

    /// Upper bound on the HTTP upgrade over an already-connected stream.
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    const UPGRADE_TIMEOUT: Duration = Duration::from_secs(30);

    /// The WebSocket upgrade over a connected, non-blocking `stream`:
    /// tungstenite reports `Interrupted` each time the stream would block,
    /// and resumes from where it stopped.
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    fn client_handshake<S: Read + Write>(
        url: &str,
        stream: S,
    ) -> Result<tungstenite::WebSocket<S>, Error> {
        use tungstenite::handshake::HandshakeError;
        let deadline = std::time::Instant::now() + UPGRADE_TIMEOUT;
        let mut attempt = tungstenite::client(url, stream);
        loop {
            match attempt {
                Ok((socket, _resp)) => return Ok(socket),
                Err(HandshakeError::Failure(e)) => return Err(map_err(e)),
                Err(HandshakeError::Interrupted(mid)) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(Error::Timeout);
                    }
                    std::thread::sleep(POLL_INTERVAL);
                    attempt = mid.handshake();
                }
            }
        }
    }

    /// `(host, port)` of a `ws://` / `wss://` URL, the port defaulting by
    /// scheme. IPv6 literals lose their brackets.
    #[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
    fn host_port(url: &str) -> Result<(String, u16), Error> {
        let uri: tungstenite::http::Uri = url
            .parse()
            .map_err(|e| Error::InvalidUrl(format!("{url}: {e}")))?;
        let host = uri
            .host()
            .ok_or_else(|| Error::InvalidUrl(format!("{url}: no host")))?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        let port = match (uri.port_u16(), uri.scheme_str()) {
            (Some(p), _) => p,
            (None, Some("wss")) => 443,
            (None, Some("ws")) => 80,
            (None, other) => {
                return Err(Error::InvalidUrl(format!("{url}: unsupported scheme {other:?}")))
            }
        };
        Ok((host, port))
    }

    /// Drive a connected socket until it closes. `ready_tx` resolves the
    /// caller's `connect`.
    fn run<S: Read + Write>(
        mut socket: tungstenite::WebSocket<S>,
        out_rx: std_mpsc::Receiver<Outbound>,
        in_tx: fut_mpsc::UnboundedSender<Result<WsMessage, Error>>,
        ready_tx: oneshot::Sender<Result<(), Error>>,
        closed: Arc<AtomicBool>,
        status: CloseSlot,
    ) {
        // A locally-initiated close sends a close frame with no status
        // code, which is what a browser reports for it too (1005).
        let local_close = || WsClose::new(WsClose::NO_STATUS, "");
        if ready_tx.send(Ok(())).is_err() {
            // Caller went away before the handshake finished.
            let _ = socket.close(None);
            return;
        }

        loop {
            if closed.load(Ordering::Relaxed) {
                let _ = socket.close(None);
                let _ = socket.flush();
                status.record(local_close());
                break;
            }

            // Drain outbound. `write` buffers; `flush` (below) drains it.
            let mut disconnected = false;
            loop {
                match out_rx.try_recv() {
                    Ok(Outbound::Msg(m)) => {
                        if let Err(e) = socket.write(to_tung(m)) {
                            if !is_would_block(&e) {
                                let _ = in_tx.unbounded_send(Err(map_err(e)));
                            }
                        }
                    }
                    Ok(Outbound::Close) => {
                        let _ = socket.close(None);
                        let _ = socket.flush();
                        status.record(local_close());
                        return;
                    }
                    Err(std_mpsc::TryRecvError::Empty) => break,
                    Err(std_mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            if disconnected {
                let _ = socket.close(None);
                let _ = socket.flush();
                status.record(local_close());
                break;
            }
            // Drain the write buffer; WouldBlock just means "more next loop".
            if let Err(e) = socket.flush() {
                if !is_would_block(&e) {
                    let _ = in_tx.unbounded_send(Err(map_err(e)));
                    break;
                }
            }

            // Read whatever is ready.
            match socket.read() {
                Ok(Message::Close(frame)) => {
                    // The peer's verdict. tungstenite queues the echo
                    // itself; the next read reports ConnectionClosed.
                    status.record(match frame {
                        Some(f) => WsClose::new(u16::from(f.code), f.reason.to_string()),
                        None => WsClose::new(WsClose::NO_STATUS, ""),
                    });
                    continue;
                }
                Ok(msg) => {
                    if let Some(m) = from_tung(msg) {
                        if in_tx.unbounded_send(Ok(m)).is_err() {
                            // Receiver dropped → nobody's listening.
                            let _ = socket.close(None);
                            break;
                        }
                    }
                    // Got a message; loop immediately to drain any more.
                    continue;
                }
                Err(tungstenite::Error::Io(ref e))
                    if e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    // Nothing ready — fall through to the idle sleep.
                }
                Err(tungstenite::Error::ConnectionClosed)
                | Err(tungstenite::Error::AlreadyClosed) => break,
                Err(e) => {
                    let _ = in_tx.unbounded_send(Err(map_err(e)));
                    break;
                }
            }

            std::thread::sleep(POLL_INTERVAL);
        }
        // Anything that got here without a close frame — a transport
        // error, the peer vanishing — is the abnormal closure a browser
        // reports as 1006. A no-op when a frame was already recorded.
        status.record(WsClose::new(WsClose::ABNORMAL, ""));
        // Dropping `in_tx` here resolves the consumer's `recv()` to `None`.
    }

    fn set_nonblocking(
        socket: &mut tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
    ) -> std::io::Result<()> {
        match socket.get_mut() {
            MaybeTlsStream::Plain(s) => s.set_nonblocking(true),
            // `wss://` via rustls: set non-blocking on the underlying TCP
            // socket beneath the TLS layer. The `Rustls` variant only
            // exists where tungstenite's TLS feature is on (everywhere but
            // Android, whose `wss://` never reaches here).
            #[cfg(not(target_os = "android"))]
            MaybeTlsStream::Rustls(s) => s.get_ref().set_nonblocking(true),
            // Non-exhaustive enum; any other variant defaults to blocking.
            _ => Ok(()),
        }
    }

    fn is_would_block(e: &tungstenite::Error) -> bool {
        matches!(e, tungstenite::Error::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock)
    }

    fn to_tung(m: WsMessage) -> Message {
        match m {
            WsMessage::Text(s) => Message::Text(s),
            WsMessage::Binary(b) => Message::Binary(b),
        }
    }

    fn from_tung(m: Message) -> Option<WsMessage> {
        match m {
            Message::Text(s) => Some(WsMessage::Text(s)),
            Message::Binary(b) => Some(WsMessage::Binary(b)),
            // Ping/Pong/Close/Frame are transport-level; tungstenite
            // auto-replies to pings, and Close is followed by a
            // ConnectionClosed on the next read.
            _ => None,
        }
    }

    fn map_err(e: tungstenite::Error) -> Error {
        use tungstenite::Error as T;
        match e {
            T::Io(io) => Error::Network(io.to_string()),
            T::Url(u) => Error::InvalidUrl(u.to_string()),
            T::Http(resp) => Error::Status {
                code: resp.status().as_u16(),
                body: None,
            },
            T::ConnectionClosed | T::AlreadyClosed => {
                Error::Network("connection closed".into())
            }
            other => Error::Other(other.to_string()),
        }
    }
    /// The Android `wss://` path minus JNI: a TCP stream whose reads go
    /// through `ws_pump`'s reader thread (as the platform TLS socket's do),
    /// the resumable upgrade, and the I/O loop.
    #[cfg(test)]
    mod pumped_tests {
        use super::*;
        use std::net::TcpListener;

        fn echo_server() -> String {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            std::thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                let mut ws = tungstenite::accept(stream).unwrap();
                while let Ok(msg) = ws.read() {
                    if msg.is_text() || msg.is_binary() {
                        ws.send(msg).unwrap();
                    }
                }
            });
            format!("ws://{addr}")
        }

        fn pumped_tcp(url: &str) -> crate::ws_pump::PumpedStream<TcpStream> {
            let (host, port) = host_port(url).unwrap();
            let tcp = TcpStream::connect((host.as_str(), port)).unwrap();
            crate::ws_pump::pumped(tcp.try_clone().unwrap(), tcp).unwrap()
        }

        #[test]
        fn a_pumped_stream_upgrades_and_echoes_through_the_io_loop() {
            let url = echo_server();
            let socket = client_handshake(&url, pumped_tcp(&url)).unwrap();

            let (out_tx, out_rx) = std_mpsc::channel();
            let (in_tx, mut in_rx) = fut_mpsc::unbounded();
            let (ready_tx, ready_rx) = oneshot::channel();
            let closed = Arc::new(AtomicBool::new(false));
            let status = CloseSlot::default();
            let (c, st) = (closed.clone(), status.clone());
            let worker = std::thread::spawn(move || run(socket, out_rx, in_tx, ready_tx, c, st));

            tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
                ready_rx.await.unwrap().unwrap();
                out_tx.send(Outbound::Msg(WsMessage::Text("over tls".into()))).unwrap();
                out_tx.send(Outbound::Msg(WsMessage::Binary(vec![0, 255, 7]))).unwrap();
                assert_eq!(in_rx.next().await.unwrap().unwrap(), WsMessage::Text("over tls".into()));
                assert_eq!(in_rx.next().await.unwrap().unwrap(), WsMessage::Binary(vec![0, 255, 7]));
            });
            out_tx.send(Outbound::Close).unwrap();
            worker.join().unwrap();
            assert_eq!(status.get().unwrap().code, WsClose::NO_STATUS);
        }

        #[test]
        fn an_upgrade_the_server_refuses_fails_instead_of_spinning() {
            // A server that answers HTTP, not a WebSocket upgrade.
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut req = [0u8; 1024];
                let _ = stream.read(&mut req);
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n");
            });
            let url = format!("ws://{addr}");
            let r = client_handshake(&url, pumped_tcp(&url));
            assert!(matches!(r, Err(Error::Status { code: 404, .. })), "{:?}", r.err());
        }

        #[test]
        fn host_and_port_default_by_scheme() {
            assert_eq!(host_port("wss://echo.example.com/x").unwrap(), ("echo.example.com".into(), 443));
            assert_eq!(host_port("ws://h/").unwrap(), ("h".into(), 80));
            assert_eq!(host_port("wss://h:8443/s?q=1").unwrap(), ("h".into(), 8443));
            assert_eq!(host_port("wss://[::1]:9/").unwrap(), ("::1".into(), 9));
            assert!(matches!(host_port("https://h/"), Err(Error::InvalidUrl(_))));
            assert!(matches!(host_port("not a url"), Err(Error::InvalidUrl(_))));
        }
    }
}

// ---------------------------------------------------------------------------
// Web arm: the browser's WebSocket through web-glue (callback-driven, no
// Rust runtime).
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::{WsClose, WsMessage};
    use crate::error::Error;
    use crate::web_glue_events as js;

    use std::cell::RefCell;
    use std::rc::Rc;

    use futures_channel::mpsc as fut_mpsc;
    use futures_channel::oneshot;
    use futures_util::StreamExt;
    use web_glue::{Closure, JsError, JsValue};

    /// Single inbound sender, shared by the event closures. `onclose`
    /// drops it (sets `None`) so the consumer's `recv()` yields `None`.
    type SenderCell = Rc<RefCell<Option<fut_mpsc::UnboundedSender<Result<WsMessage, Error>>>>>;

    pub struct WebSocketImpl {
        ws: JsValue,
        inbound: fut_mpsc::UnboundedReceiver<Result<WsMessage, Error>>,
        /// Written by `onclose` BEFORE it drops the inbound sender, so
        /// it is set by the time `recv` yields `None`.
        close_status: Rc<RefCell<Option<WsClose>>>,
        // Closures must outlive the socket so the browser can call them
        // — and must be DETACHED from it before they die. See `Handlers`.
        _handlers: Handlers,
    }

    /// Owns the socket's event closures and clears the JS handler slots
    /// when they die.
    ///
    /// Dropping a `Closure` revokes the JS function that forwards into
    /// wasm, but it does not unregister anything: the slot on the JS
    /// `WebSocket` still points at the dead function. A socket that can
    /// still emit — one whose handshake just failed, or one dropped
    /// while open — then throws `called after its Rust owner dropped it`
    /// into the event loop on its next `error`/`close`. It does not trap
    /// the module, so nothing user-visible breaks; it buries the console
    /// in exceptions on exactly the connections someone is debugging.
    ///
    /// Tying the detach to the closures' own `Drop` makes the mistake
    /// unrepresentable rather than path-by-path: `connect`'s `?` early
    /// return, `WebSocketImpl`'s teardown and a panic between them all
    /// clear the slots on the way out.
    ///
    /// Regression: `tests/web_closure_lifetime.rs` (browser).
    struct Handlers {
        ws: JsValue,
        _onmessage: Closure,
        _onclose: Closure,
        _onerror: Closure,
    }

    impl Drop for Handlers {
        fn drop(&mut self) {
            js::set_handler(&self.ws, "onmessage", None);
            js::set_handler(&self.ws, "onclose", None);
            js::set_handler(&self.ws, "onerror", None);
            // `onopen` is dropped as soon as the handshake resolves, so
            // its slot can be stale too.
            js::set_handler(&self.ws, "onopen", None);
        }
    }

    pub async fn connect(url: &str) -> Result<WebSocketImpl, Error> {
        let ws = js::websocket_new(url).map_err(js_err)?;

        let (in_tx, in_rx) = fut_mpsc::unbounded::<Result<WsMessage, Error>>();
        let sender: SenderCell = Rc::new(RefCell::new(Some(in_tx)));
        let (open_tx, open_rx) = oneshot::channel::<Result<(), Error>>();
        let open_tx = Rc::new(RefCell::new(Some(open_tx)));

        // onmessage → decode + push into the inbound channel.
        let onmessage = {
            let sender = sender.clone();
            Closure::new(move |e: JsValue| {
                let msg = if let Some(txt) = js::data_text(&e) {
                    Some(WsMessage::Text(txt))
                } else {
                    js::data_bytes(&e).map(WsMessage::Binary)
                };
                if let (Some(m), Some(tx)) = (msg, sender.borrow().as_ref()) {
                    let _ = tx.unbounded_send(Ok(m));
                }
            })
        };
        js::set_handler(&ws, "onmessage", Some(&onmessage));

        // onopen → resolve `connect` once.
        let onopen = {
            let open_tx = open_tx.clone();
            Closure::new(move |_| {
                if let Some(t) = open_tx.borrow_mut().take() {
                    let _ = t.send(Ok(()));
                }
            })
        };
        js::set_handler(&ws, "onopen", Some(&onopen));

        // onclose → record how it ended, then drop the sender so
        // `recv()` ends with `None`. The browser already synthesizes
        // 1005/1006 for the no-frame cases, which is the contract.
        let close_status: Rc<RefCell<Option<WsClose>>> = Rc::new(RefCell::new(None));
        let onclose = {
            let sender = sender.clone();
            let close_status = close_status.clone();
            Closure::new(move |e: JsValue| {
                let (code, reason) = js::close_code_and_reason(&e);
                close_status
                    .borrow_mut()
                    .get_or_insert_with(|| WsClose::new(code, reason));
                *sender.borrow_mut() = None;
            })
        };
        js::set_handler(&ws, "onclose", Some(&onclose));

        // onerror → fail `connect` if still pending; otherwise it precedes
        // an onclose which ends the stream.
        let onerror = {
            let open_tx = open_tx.clone();
            Closure::new(move |_| {
                if let Some(t) = open_tx.borrow_mut().take() {
                    let _ = t.send(Err(Error::Network("websocket error".into())));
                }
            })
        };
        js::set_handler(&ws, "onerror", Some(&onerror));

        let handlers = Handlers {
            ws: ws.clone(),
            _onmessage: onmessage,
            _onclose: onclose,
            _onerror: onerror,
        };

        // Await the handshake. `onopen` is only needed until this resolves.
        let result = open_rx
            .await
            .unwrap_or_else(|_| Err(Error::Network("websocket open cancelled".into())));
        js::set_handler(&ws, "onopen", None);
        drop(onopen);
        if let Err(e) = result {
            // The handshake failed, but the JS socket is still live and
            // WILL deliver `close` after this `error`. Detach first, then
            // close, so that event finds an empty slot instead of the
            // revoked functions of the closures we are about to drop.
            drop(handlers);
            js::close(&ws);
            return Err(e);
        }

        Ok(WebSocketImpl {
            ws,
            inbound: in_rx,
            close_status,
            _handlers: handlers,
        })
    }

    impl WebSocketImpl {
        pub fn send(&self, msg: WsMessage) -> Result<(), Error> {
            send_on(&self.ws, msg)
        }
        pub async fn recv(&mut self) -> Option<Result<WsMessage, Error>> {
            self.inbound.next().await
        }
        pub fn close(&self) {
            js::close(&self.ws);
        }
        pub fn close_status(&self) -> Option<WsClose> {
            self.close_status.borrow().clone()
        }
        pub fn sender(&self) -> WsSenderImpl {
            WsSenderImpl {
                ws: self.ws.clone(),
            }
        }
    }

    /// Close-on-drop, matching the type's documented contract and the
    /// native arm's `Drop`. Closing here happens BEFORE `handlers` drops
    /// (fields drop after the body), so the detach still lands ahead of
    /// the close event.
    impl Drop for WebSocketImpl {
        fn drop(&mut self) {
            js::close(&self.ws);
        }
    }

    /// Cloneable send handle — a clone of the JS `WebSocket` (a handle).
    #[derive(Clone)]
    pub struct WsSenderImpl {
        ws: JsValue,
    }

    impl WsSenderImpl {
        pub fn send(&self, msg: WsMessage) -> Result<(), Error> {
            send_on(&self.ws, msg)
        }
        pub fn close(&self) {
            js::close(&self.ws);
        }
    }

    fn send_on(ws: &JsValue, msg: WsMessage) -> Result<(), Error> {
        match msg {
            WsMessage::Text(s) => js::send_text(ws, &s).map_err(js_err),
            WsMessage::Binary(b) => js::send_bytes(ws, &b).map_err(js_err),
        }
    }

    fn js_err(e: JsError) -> Error {
        Error::Network(e.message())
    }
}

// iOS and Android use the native `tungstenite` arm above (they're native
// Rust targets with TCP sockets + threads). A platform-native arm
// (`URLSessionWebSocketTask` / OkHttp) for OS proxy/background integration
// is a documented follow-on, not a correctness gap.
