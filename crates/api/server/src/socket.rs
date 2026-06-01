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
