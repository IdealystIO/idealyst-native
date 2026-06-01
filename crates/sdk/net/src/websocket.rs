//! Cross-platform WebSocket client — sibling to the HTTP [`Client`](crate::Client).
//!
//! One async surface (`connect` / `send` / `recv` / `close`, plus
//! close-on-drop) that maps to the platform-native socket on each target,
//! exactly like the HTTP client maps to fetch / NSURLSession /
//! HttpURLConnection / reqwest:
//!
//! | target            | backend                          |
//! |-------------------|----------------------------------|
//! | web (wasm32)      | `web_sys::WebSocket`             |
//! | iOS / macOS / tvOS| `URLSessionWebSocketTask`        |
//! | Android           | OkHttp `WebSocket` (JNI)         |
//! | native / terminal | sync `tungstenite` on an I/O thread |
//!
//! # Execution model
//!
//! Per the framework's runtime invariant, this introduces **no async
//! runtime**. On native, a single blocking worker thread owns the socket
//! and does the reads/writes; inbound messages are bridged to the async
//! `recv()` through a `futures-channel` whose cross-thread waker re-polls
//! under the framework's scheduler. On web / Apple / Android the OS event
//! loop is the runtime and callbacks marshal in the same way. Nothing
//! here spins up tokio.
//!
//! Only the native arm is implemented today; the other targets return an
//! "unimplemented" error so the crate still compiles everywhere.

use crate::error::Error;

/// A WebSocket message. Control frames (ping/pong/close) are handled by
/// the transport and never surface here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
}

/// A connected WebSocket. The connection is closed when this is dropped
/// (so a `use_socket`-style hook gets teardown for free by tying the
/// handle's lifetime to a component scope).
pub struct WebSocket {
    inner: imp::WebSocketImpl,
}

impl WebSocket {
    /// Open a connection to `url` (`ws://…`; `wss://` once the TLS feature
    /// lands). Resolves once the handshake completes.
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
    use super::WsMessage;
    use crate::error::Error;

    use std::net::TcpStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc as std_mpsc;
    use std::sync::Arc;
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
    }

    pub async fn connect(url: &str) -> Result<WebSocketImpl, Error> {
        let (out_tx, out_rx) = std_mpsc::channel::<Outbound>();
        let (in_tx, in_rx) = fut_mpsc::unbounded::<Result<WsMessage, Error>>();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), Error>>();
        let closed = Arc::new(AtomicBool::new(false));

        let url = url.to_string();
        let closed_thread = closed.clone();
        std::thread::Builder::new()
            .name("net-ws".into())
            .spawn(move || io_loop(url, out_rx, in_tx, ready_tx, closed_thread))
            .map_err(|e| Error::Other(format!("ws thread spawn failed: {e}")))?;

        // The handshake runs on the worker thread; await its result so
        // `connect` only resolves once the socket is live.
        match ready_rx.await {
            Ok(Ok(())) => Ok(WebSocketImpl {
                outbound: out_tx,
                inbound: in_rx,
                closed,
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
    ) {
        // Blocking handshake.
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
        if ready_tx.send(Ok(())).is_err() {
            // Caller went away before the handshake finished.
            let _ = socket.close(None);
            return;
        }

        loop {
            if closed.load(Ordering::Relaxed) {
                let _ = socket.close(None);
                let _ = socket.flush();
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
            // Android — see Cargo.toml).
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
}

// ---------------------------------------------------------------------------
// Web arm: web_sys::WebSocket (callback-driven, no Rust runtime).
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::WsMessage;
    use crate::error::Error;

    use std::cell::RefCell;
    use std::rc::Rc;

    use futures_channel::mpsc as fut_mpsc;
    use futures_channel::oneshot;
    use futures_util::StreamExt;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket as WebSysWs};

    /// Single inbound sender, shared by the event closures. `onclose`
    /// drops it (sets `None`) so the consumer's `recv()` yields `None`.
    type SenderCell = Rc<RefCell<Option<fut_mpsc::UnboundedSender<Result<WsMessage, Error>>>>>;

    pub struct WebSocketImpl {
        ws: WebSysWs,
        inbound: fut_mpsc::UnboundedReceiver<Result<WsMessage, Error>>,
        // Closures must outlive the socket so the browser can call them.
        _onmessage: Closure<dyn FnMut(MessageEvent)>,
        _onclose: Closure<dyn FnMut(CloseEvent)>,
        _onerror: Closure<dyn FnMut(Event)>,
    }

    pub async fn connect(url: &str) -> Result<WebSocketImpl, Error> {
        let ws = WebSysWs::new(url).map_err(js_err)?;
        ws.set_binary_type(BinaryType::Arraybuffer);

        let (in_tx, in_rx) = fut_mpsc::unbounded::<Result<WsMessage, Error>>();
        let sender: SenderCell = Rc::new(RefCell::new(Some(in_tx)));
        let (open_tx, open_rx) = oneshot::channel::<Result<(), Error>>();
        let open_tx = Rc::new(RefCell::new(Some(open_tx)));

        // onmessage → decode + push into the inbound channel.
        let onmessage = {
            let sender = sender.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                let data = e.data();
                let msg = if let Some(txt) = data.as_string() {
                    Some(WsMessage::Text(txt))
                } else if let Ok(buf) = data.dyn_into::<js_sys::ArrayBuffer>() {
                    Some(WsMessage::Binary(js_sys::Uint8Array::new(&buf).to_vec()))
                } else {
                    None
                };
                if let (Some(m), Some(tx)) = (msg, sender.borrow().as_ref()) {
                    let _ = tx.unbounded_send(Ok(m));
                }
            })
        };
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

        // onopen → resolve `connect` once.
        let onopen = {
            let open_tx = open_tx.clone();
            Closure::<dyn FnMut(Event)>::new(move |_| {
                if let Some(t) = open_tx.borrow_mut().take() {
                    let _ = t.send(Ok(()));
                }
            })
        };
        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));

        // onclose → drop the sender so `recv()` ends with `None`.
        let onclose = {
            let sender = sender.clone();
            Closure::<dyn FnMut(CloseEvent)>::new(move |_| {
                *sender.borrow_mut() = None;
            })
        };
        ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));

        // onerror → fail `connect` if still pending; otherwise it precedes
        // an onclose which ends the stream.
        let onerror = {
            let open_tx = open_tx.clone();
            Closure::<dyn FnMut(Event)>::new(move |_| {
                if let Some(t) = open_tx.borrow_mut().take() {
                    let _ = t.send(Err(Error::Network("websocket error".into())));
                }
            })
        };
        ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));

        // Await the handshake. `onopen` is only needed until this resolves.
        let result = open_rx
            .await
            .unwrap_or_else(|_| Err(Error::Network("websocket open cancelled".into())));
        drop(onopen);
        result?;

        Ok(WebSocketImpl {
            ws,
            inbound: in_rx,
            _onmessage: onmessage,
            _onclose: onclose,
            _onerror: onerror,
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
            let _ = self.ws.close();
        }
        pub fn sender(&self) -> WsSenderImpl {
            WsSenderImpl {
                ws: self.ws.clone(),
            }
        }
    }

    /// Cloneable send handle — a clone of the JS `WebSocket` (a handle).
    #[derive(Clone)]
    pub struct WsSenderImpl {
        ws: WebSysWs,
    }

    impl WsSenderImpl {
        pub fn send(&self, msg: WsMessage) -> Result<(), Error> {
            send_on(&self.ws, msg)
        }
        pub fn close(&self) {
            let _ = self.ws.close();
        }
    }

    fn send_on(ws: &WebSysWs, msg: WsMessage) -> Result<(), Error> {
        match msg {
            WsMessage::Text(s) => ws.send_with_str(&s).map_err(js_err),
            WsMessage::Binary(b) => ws.send_with_u8_array(&b).map_err(js_err),
        }
    }

    fn js_err(e: JsValue) -> Error {
        Error::Network(format!("{e:?}"))
    }
}

// iOS and Android use the native `tungstenite` arm above (they're native
// Rust targets with TCP sockets + threads). A platform-native arm
// (`URLSessionWebSocketTask` / OkHttp) for OS proxy/background integration
// is a documented follow-on, not a correctness gap.
