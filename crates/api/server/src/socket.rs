//! `Socket<In, Out>` — a typed WebSocket over JSON frames.
//!
//! The streaming counterpart to a `#[server]` fn: instead of one request
//! → one `Result`, it's a live duplex where the **message enums are the
//! contract**. Because client and server are one project compiled via cfg
//! gates, both sides share the same `In`/`Out` types — there's no
//! protocol to keep in sync.
//!
//! `Socket<In, Out>` means "I receive `In`, I send `Out`", so the two
//! ends mirror: the client holds `Socket<ServerMsg, ClientMsg>`, the
//! server handler holds `Socket<ClientMsg, ServerMsg>`.
//!
//! It cfg-splits exactly like the rest of the SDK:
//! - **client build**: wraps [`net::WebSocket`] (the per-platform socket).
//! - **server build**: wraps the axum WebSocket from an upgrade.
//!
//! The frame format is JSON text (matching the HTTP layer); binary
//! frames are accepted on recv for forward-compat with a postcard codec.
//!
//! This slice gives the typed transport + the server `accept` helper. The
//! `#[channel]`/`#[subscription]` macros and the `use_socket` reactive
//! hook build on top of it.

use std::marker::PhantomData;

/// A streaming transport failure.
#[derive(Debug)]
pub enum SocketError {
    /// Underlying socket failure (connect, send, the connection dropping).
    Transport(String),
    /// A frame failed to encode/decode into the message type.
    Codec(String),
}

impl std::fmt::Display for SocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SocketError::Transport(m) => write!(f, "socket transport error: {m}"),
            SocketError::Codec(m) => write!(f, "socket codec error: {m}"),
        }
    }
}

impl std::error::Error for SocketError {}

/// A typed, bidirectional WebSocket. Receives `In`, sends `Out`. Closes
/// on drop (so a scope-owned handle tears the connection down for free).
///
/// `send`/`recv` both take `&mut self` in this slice, so a handler does
/// recv-then-send sequentially (echo, request/reply, subscription).
/// Concurrent duplex via a `split()` into send/recv halves is a follow-on.
pub struct Socket<In, Out> {
    inner: Inner,
    // Covariant, always Send+Sync regardless of In/Out (so the socket can
    // cross into an axum task); the real bounds live on the methods.
    _marker: PhantomData<fn() -> (In, Out)>,
}

// ---------------------------------------------------------------------------
// Client build: wraps net::WebSocket.
// ---------------------------------------------------------------------------

#[cfg(not(feature = "server"))]
struct Inner(net::WebSocket);

#[cfg(not(feature = "server"))]
impl<In, Out> Socket<In, Out>
where
    In: serde::de::DeserializeOwned,
    Out: serde::Serialize,
{
    /// Open a typed connection to `url` (`ws://…`).
    pub async fn connect(url: &str) -> Result<Self, SocketError> {
        let ws = net::WebSocket::connect(url)
            .await
            .map_err(|e| SocketError::Transport(e.to_string()))?;
        Ok(Self {
            inner: Inner(ws),
            _marker: PhantomData,
        })
    }

    /// Encode and queue `msg`. Returns once queued (the write happens on
    /// the transport's I/O source); `async` to mirror the server side.
    pub async fn send(&mut self, msg: Out) -> Result<(), SocketError> {
        let json = serde_json::to_string(&msg).map_err(|e| SocketError::Codec(e.to_string()))?;
        self.inner
            .0
            .send(net::WsMessage::Text(json))
            .map_err(|e| SocketError::Transport(e.to_string()))
    }

    /// Await and decode the next inbound message. `None` = closed.
    pub async fn recv(&mut self) -> Option<Result<In, SocketError>> {
        match self.inner.0.recv().await {
            Some(Ok(net::WsMessage::Text(s))) => Some(decode(s.as_bytes())),
            Some(Ok(net::WsMessage::Binary(b))) => Some(decode(&b)),
            Some(Err(e)) => Some(Err(SocketError::Transport(e.to_string()))),
            None => None,
        }
    }

    /// A cloneable [`SocketSender`] — so a UI scope can send while a recv
    /// loop owns the socket (`recv` needs `&mut self`). Powers [`use_socket`].
    pub fn sender(&self) -> SocketSender<Out> {
        SocketSender {
            inner: self.inner.0.sender(),
            _marker: PhantomData,
        }
    }
}

// ---------------------------------------------------------------------------
// Server build: wraps the axum WebSocket.
// ---------------------------------------------------------------------------

#[cfg(feature = "server")]
struct Inner(axum::extract::ws::WebSocket);

#[cfg(feature = "server")]
impl<In, Out> Socket<In, Out>
where
    In: serde::de::DeserializeOwned,
    Out: serde::Serialize,
{
    fn from_axum(ws: axum::extract::ws::WebSocket) -> Self {
        Self {
            inner: Inner(ws),
            _marker: PhantomData,
        }
    }

    /// Encode and send `msg` to the peer.
    pub async fn send(&mut self, msg: Out) -> Result<(), SocketError> {
        let json = serde_json::to_string(&msg).map_err(|e| SocketError::Codec(e.to_string()))?;
        self.inner
            .0
            .send(axum::extract::ws::Message::Text(json))
            .await
            .map_err(|e| SocketError::Transport(e.to_string()))
    }

    /// Await and decode the next inbound message. `None` = closed.
    /// Control frames (ping/pong/close) are skipped.
    pub async fn recv(&mut self) -> Option<Result<In, SocketError>> {
        use axum::extract::ws::Message;
        loop {
            match self.inner.0.recv().await {
                Some(Ok(Message::Text(s))) => return Some(decode(s.as_bytes())),
                Some(Ok(Message::Binary(b))) => return Some(decode(&b)),
                Some(Ok(_)) => continue, // ping / pong / close → skip
                Some(Err(e)) => return Some(Err(SocketError::Transport(e.to_string()))),
                None => return None,
            }
        }
    }
}

/// Upgrade an incoming request to a typed WebSocket and run `handler`
/// with the resulting [`Socket`]. Mount it on a route:
///
/// ```ignore
/// async fn chat_ws(ws: axum::extract::ws::WebSocketUpgrade) -> axum::response::Response {
///     server::accept(ws, |mut sock: Socket<ClientMsg, ServerMsg>| async move {
///         while let Some(Ok(msg)) = sock.recv().await { sock.send(reply(msg)).await.ok(); }
///     })
/// }
/// // router().route("/_srv/_ws/chat", axum::routing::get(chat_ws))
/// ```
///
/// The `#[channel]` macro will generate this wrapper; until then authors
/// write it by hand.
#[cfg(feature = "server")]
pub fn accept<In, Out, F, Fut>(
    ws: axum::extract::ws::WebSocketUpgrade,
    handler: F,
) -> axum::response::Response
where
    In: serde::de::DeserializeOwned + Send + 'static,
    Out: serde::Serialize + Send + 'static,
    F: FnOnce(Socket<In, Out>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    ws.on_upgrade(move |socket| async move {
        handler(Socket::from_axum(socket)).await;
    })
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, SocketError> {
    serde_json::from_slice(bytes).map_err(|e| SocketError::Codec(e.to_string()))
}

// ---------------------------------------------------------------------------
// Client-only: the cloneable send half + the `use_socket` reactive hook.
// ---------------------------------------------------------------------------

/// A cloneable send handle for a client [`Socket`]. Sending is
/// independent of the receive loop, so the UI scope can hold this while a
/// spawned task owns the socket for `recv`.
#[cfg(not(feature = "server"))]
pub struct SocketSender<Out> {
    inner: net::WsSender,
    _marker: PhantomData<fn(Out)>,
}

#[cfg(not(feature = "server"))]
impl<Out> Clone for SocketSender<Out> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _marker: PhantomData,
        }
    }
}

#[cfg(not(feature = "server"))]
impl<Out: serde::Serialize> SocketSender<Out> {
    /// Encode and queue `msg`.
    pub fn send(&self, msg: Out) -> Result<(), SocketError> {
        let json = serde_json::to_string(&msg).map_err(|e| SocketError::Codec(e.to_string()))?;
        self.inner
            .send(net::WsMessage::Text(json))
            .map_err(|e| SocketError::Transport(e.to_string()))
    }

    /// Close the connection.
    pub fn close(&self) {
        self.inner.close();
    }
}

/// Lifecycle of a [`use_socket`] connection.
#[cfg(not(feature = "server"))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SocketStatus {
    Connecting,
    Open,
    Closed,
    Error,
}

/// Coordinates teardown between the component scope and the spawned recv
/// loop: `on_cleanup` (unmount) sets `cancelled` and closes the live
/// sender; the loop, once connected, registers its sender here (and bails
/// immediately if the scope already unmounted before the connect landed).
#[cfg(not(feature = "server"))]
struct CloseCoord<Out> {
    cancelled: bool,
    sender: Option<SocketSender<Out>>,
}

#[cfg(not(feature = "server"))]
impl<Out: serde::Serialize> CloseCoord<Out> {
    fn close(&mut self) {
        self.cancelled = true;
        if let Some(s) = &self.sender {
            s.close();
        }
    }
}

/// The reactive handle returned by [`use_socket`]. Cheap (`Copy`) — it's
/// three signal ids — so clone it freely into closures.
#[cfg(not(feature = "server"))]
pub struct UseSocket<In, Out> {
    incoming: runtime_core::Signal<Option<In>>,
    status: runtime_core::Signal<SocketStatus>,
    sender: runtime_core::Signal<Option<SocketSender<Out>>>,
}

#[cfg(not(feature = "server"))]
impl<In, Out> Clone for UseSocket<In, Out> {
    fn clone(&self) -> Self {
        *self
    }
}

#[cfg(not(feature = "server"))]
impl<In, Out> Copy for UseSocket<In, Out> {}

#[cfg(not(feature = "server"))]
impl<In: Clone + 'static, Out: serde::Serialize + 'static> UseSocket<In, Out> {
    /// The latest-message signal — read it in `ui!`/`rx!` to re-render on
    /// each inbound message. `None` until the first arrives.
    pub fn incoming(&self) -> runtime_core::Signal<Option<In>> {
        self.incoming
    }

    /// The latest received message, if any (a non-reactive read).
    pub fn latest(&self) -> Option<In> {
        self.incoming.get()
    }

    /// The connection's current [`SocketStatus`] (reactive read).
    pub fn status(&self) -> SocketStatus {
        self.status.get()
    }

    /// Send a message. Returns `false` if not yet connected or closed.
    pub fn send(&self, msg: Out) -> bool {
        match self.sender.get() {
            Some(tx) => tx.send(msg).is_ok(),
            None => false,
        }
    }
}

/// Open a typed WebSocket bound to the current component scope: it
/// connects on mount and **closes on unmount**, with no teardown code —
/// `on_cleanup` (scope drop) closes the socket, which ends the spawned
/// recv loop. Inbound messages land in the reactive `incoming()` signal.
///
/// ```ignore
/// #[component]
/// fn live_tasks() -> Element {
///     let sock = use_socket::<ServerMsg, ClientMsg>("ws://…/_srv/_ws/tasks");
///     sock.send(ClientMsg::Subscribe);
///     ui! { text(move || format!("{:?}", sock.incoming().get())) }
/// }
/// ```
///
/// `Socket<In, Out>` mirrors as always — the client receives `In`
/// (`ServerMsg`) and sends `Out` (`ClientMsg`).
#[cfg(not(feature = "server"))]
pub fn use_socket<In, Out>(url: impl Into<String>) -> UseSocket<In, Out>
where
    In: serde::de::DeserializeOwned + Clone + 'static,
    Out: serde::Serialize + 'static,
{
    use std::cell::RefCell;
    use std::rc::Rc;

    let incoming: runtime_core::Signal<Option<In>> = runtime_core::signal!(None);
    let status: runtime_core::Signal<SocketStatus> = runtime_core::signal!(SocketStatus::Connecting);
    let sender: runtime_core::Signal<Option<SocketSender<Out>>> = runtime_core::signal!(None);

    let coord: Rc<RefCell<CloseCoord<Out>>> = Rc::new(RefCell::new(CloseCoord {
        cancelled: false,
        sender: None,
    }));

    // Teardown on unmount: the scope drop fires this, which closes the
    // socket → the recv loop's `recv()` returns `None` → the task ends.
    {
        let coord = coord.clone();
        runtime_core::on_cleanup(move || coord.borrow_mut().close());
    }

    let url = url.into();
    runtime_core::driver::spawn_async(async move {
        match Socket::<In, Out>::connect(&url).await {
            Ok(mut sock) => {
                let tx = sock.sender();
                {
                    let mut c = coord.borrow_mut();
                    if c.cancelled {
                        // Unmounted before the connect landed — close now.
                        tx.close();
                        status.set(SocketStatus::Closed);
                        return;
                    }
                    c.sender = Some(tx.clone());
                }
                sender.set(Some(tx));
                status.set(SocketStatus::Open);

                while let Some(res) = sock.recv().await {
                    match res {
                        Ok(msg) => incoming.set(Some(msg)),
                        Err(_) => {
                            status.set(SocketStatus::Error);
                            return;
                        }
                    }
                }
                status.set(SocketStatus::Closed);
            }
            Err(_) => status.set(SocketStatus::Error),
        }
    });

    UseSocket {
        incoming,
        status,
        sender,
    }
}
