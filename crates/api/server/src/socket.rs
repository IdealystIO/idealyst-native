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
            inner: std::rc::Rc::new(self.inner.0.sender()),
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
    // `Rc` for IDENTITY, not for sharing: `WsSender` is already a cheap
    // cloneable handle. The world kernel bounds every `Signal<T>` on
    // `T: PartialEq`, and a socket sender has no value equality — so the
    // `PartialEq` below compares `Rc` pointers, i.e. "are these two
    // handles the same connection?", which is exactly the question the
    // guarded `set` would need to answer. (The install site uses
    // `set_always`, so a re-install notifies regardless.)
    inner: std::rc::Rc<net::WsSender>,
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

/// Pointer identity — see the `inner` field comment. Deliberately NOT
/// derived: `net::WsSender` has no value equality on either arm.
#[cfg(not(feature = "server"))]
impl<Out> PartialEq for SocketSender<Out> {
    fn eq(&self, other: &Self) -> bool {
        std::rc::Rc::ptr_eq(&self.inner, &other.inner)
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
impl<In: Clone + PartialEq + 'static, Out: serde::Serialize + 'static> UseSocket<In, Out> {
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
/// Unmounting while connected — or while still connecting — is safe at
/// any moment: the recv loop's writes are guarded by the calling scope's
/// liveness token, so the close that teardown itself performs reports
/// nothing back into the freed `status`/`incoming` slots. The hook is
/// therefore usable under a `switch` whose key can change while the
/// socket is live (an API key that hydrates from storage and rides the
/// connect URL is the case that found this).
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
    // `PartialEq` on `In` is the world kernel's `Signal<T>` bound — every
    // reactive slot carries it. Derive it alongside `Deserialize` on your
    // message enum; `set_always` below still forces a notification per
    // delivery, so two identical payloads in a row both re-render.
    In: serde::de::DeserializeOwned + Clone + PartialEq + 'static,
    Out: serde::Serialize + 'static,
{
    use std::cell::RefCell;
    use std::rc::Rc;

    let incoming: runtime_core::Signal<Option<In>> = runtime_core::signal(None);
    let status: runtime_core::Signal<SocketStatus> = runtime_core::signal(SocketStatus::Connecting);
    let sender: runtime_core::Signal<Option<SocketSender<Out>>> = runtime_core::signal(None);

    let coord: Rc<RefCell<CloseCoord<Out>>> = Rc::new(RefCell::new(CloseCoord {
        cancelled: false,
        sender: None,
    }));

    // Teardown on unmount: the scope drop fires this, which closes the
    // socket → the recv loop's `recv()` returns `None` → the task ends.
    //
    // The cleanup is RETURNED FROM AN EFFECT rather than registered with a
    // bare `on_cleanup`. `on_cleanup` requires a running effect and panics
    // ("on_cleanup called outside an effect") anywhere else — and this hook
    // is called from a component body, which is not an effect. The effect
    // body reads nothing, so it runs exactly once; its returned cleanup is
    // registered through the same mechanism `on_cleanup` uses and fires
    // when the owning scope drops.
    {
        let coord = coord.clone();
        let _ = runtime_core::effect(move || {
            let coord = coord.clone();
            move || coord.borrow_mut().close()
        });
    }

    // Liveness token for every write the driving task makes. Taken HERE,
    // in the calling component's scope (a build, so `ScopeAlive::current`
    // anchors to the subtree being built) — not inside the async block,
    // which resumes with no ambient scope to anchor to.
    //
    // The task below is DETACHED (`spawn_async` takes no token), and
    // teardown is precisely what wakes it: the cleanup above closes the
    // socket, `recv()` then yields `None`, and the continuation runs
    // against a scope whose `Owned` has already freed `incoming`,
    // `status` and `sender`. Every one of those writes then aborts the
    // module with `idealyst[stale-signal-handle]` — on web, on every
    // re-key of a `switch` that owns the hook. The token makes the
    // post-teardown writes no-ops instead, which is the correct
    // semantic: the scope that would have read them is gone.
    //
    // Per-write guarding (rather than `spawn_then`'s all-or-nothing
    // callback) is right for this shape: each write here is a single
    // independent status/message delivery, and the loop must keep
    // running across many of them.
    //
    // Regression: `tests/hook_scope_teardown.rs`.
    let alive = runtime_core::ScopeAlive::current();
    let url = url.into();
    runtime_core::driver::spawn_async(async move {
        match Socket::<In, Out>::connect(&url).await {
            Ok(mut sock) => {
                let tx = sock.sender();
                {
                    let mut c = coord.borrow_mut();
                    if c.cancelled || !alive.get() {
                        // Unmounted before the connect landed — close now.
                        // No `status` write: `cancelled` is set by the
                        // scope-drop cleanup, so the slot is already freed.
                        tx.close();
                        return;
                    }
                    c.sender = Some(tx.clone());
                }
                // `set_always`, not `set`: every delivery must notify
                // even when the payload compares equal to the previous
                // one (two identical inbound messages are two events).
                // Plain `set` is equality-guarded and would swallow the
                // second.
                sender.set_always(Some(tx));
                status.set(SocketStatus::Open);

                while let Some(res) = sock.recv().await {
                    // Re-checked per iteration, not once: teardown can
                    // land between any two deliveries, and the close it
                    // performs is what resumes this loop.
                    if !alive.get() {
                        return;
                    }
                    match res {
                        Ok(msg) => incoming.set_always(Some(msg)),
                        Err(_) => {
                            status.set(SocketStatus::Error);
                            return;
                        }
                    }
                }
                // The recv loop ends on ANY close, including the one
                // teardown just performed — this guard is what keeps that
                // ordinary path from writing a freed slot.
                if alive.get() {
                    status.set(SocketStatus::Closed);
                }
            }
            Err(_) => {
                if alive.get() {
                    status.set(SocketStatus::Error);
                }
            }
        }
    });

    UseSocket {
        incoming,
        status,
        sender,
    }
}

// ---------------------------------------------------------------------------
// Client-only: the `use_sse` reactive hook (Server-Sent Events consumer).
// ---------------------------------------------------------------------------

/// Lifecycle of a [`use_sse`] connection.
#[cfg(not(feature = "server"))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SseStatus {
    Connecting,
    Open,
    Closed,
    Error,
}

#[cfg(not(feature = "server"))]
struct SseCloseCoord {
    cancelled: bool,
    closer: Option<net::EventSourceCloser>,
}

#[cfg(not(feature = "server"))]
impl SseCloseCoord {
    fn close(&mut self) {
        self.cancelled = true;
        if let Some(c) = &self.closer {
            c.close();
        }
    }
}

/// The reactive handle from [`use_sse`]. Cheap (`Copy`) — two signal ids.
#[cfg(not(feature = "server"))]
pub struct UseSse<T> {
    incoming: runtime_core::Signal<Option<T>>,
    status: runtime_core::Signal<SseStatus>,
}

#[cfg(not(feature = "server"))]
impl<T> Clone for UseSse<T> {
    fn clone(&self) -> Self {
        *self
    }
}

#[cfg(not(feature = "server"))]
impl<T> Copy for UseSse<T> {}

#[cfg(not(feature = "server"))]
impl<T: Clone + PartialEq + 'static> UseSse<T> {
    /// The latest-event signal — read it in `ui!`/`rx!` to re-render per
    /// event. `None` until the first arrives.
    pub fn incoming(&self) -> runtime_core::Signal<Option<T>> {
        self.incoming
    }
    /// The latest decoded event, if any (non-reactive read).
    pub fn latest(&self) -> Option<T> {
        self.incoming.get()
    }
    /// The connection's current [`SseStatus`] (reactive read).
    pub fn status(&self) -> SseStatus {
        self.status.get()
    }
}

/// Subscribe to a Server-Sent Events stream (a `#[sse]` endpoint), bound
/// to the current component scope: connects on mount, **closes on
/// unmount** (`on_cleanup` → the `EventSource` closer ends the read
/// loop), with each event's `data:` payload JSON-decoded into `T` and
/// pushed to the reactive `incoming()` signal. The receive-only SSE
/// counterpart of [`use_socket`].
///
/// ```ignore
/// #[component]
/// fn notifications() -> Element {
///     let feed = use_sse::<Note>(notifications_url());   // a #[sse] client stub returns the URL
///     ui! { text(move || format!("{:?}", feed.incoming().get())) }
/// }
/// ```
#[cfg(not(feature = "server"))]
pub fn use_sse<T>(url: impl Into<String>) -> UseSse<T>
where
    // See `use_socket` — the world kernel bounds every signal payload on
    // `PartialEq`; `set_always` still notifies on every event.
    T: serde::de::DeserializeOwned + Clone + PartialEq + 'static,
{
    use std::cell::RefCell;
    use std::rc::Rc;

    let incoming: runtime_core::Signal<Option<T>> = runtime_core::signal(None);
    let status: runtime_core::Signal<SseStatus> = runtime_core::signal(SseStatus::Connecting);

    let coord = Rc::new(RefCell::new(SseCloseCoord {
        cancelled: false,
        closer: None,
    }));

    // Effect-returned cleanup, not a bare `on_cleanup` — see the
    // equivalent block in `use_socket` for why (`on_cleanup` panics
    // outside a running effect, and this hook runs in a component body).
    {
        let coord = coord.clone();
        let _ = runtime_core::effect(move || {
            let coord = coord.clone();
            move || coord.borrow_mut().close()
        });
    }

    // Same liveness token as `use_socket`, for the same reason — the
    // driving task is detached and the scope drop that closes the stream
    // is what resumes it. See `use_socket` for the full rationale.
    //
    // Regression: `tests/hook_scope_teardown.rs`.
    let alive = runtime_core::ScopeAlive::current();
    let url = url.into();
    runtime_core::driver::spawn_async(async move {
        match net::EventSource::connect(&url).await {
            Ok(mut es) => {
                let closer = es.closer();
                {
                    let mut c = coord.borrow_mut();
                    if c.cancelled || !alive.get() {
                        closer.close();
                        return;
                    }
                    c.closer = Some(closer);
                }
                status.set(SseStatus::Open);

                while let Some(res) = es.recv().await {
                    if !alive.get() {
                        return;
                    }
                    match res {
                        Ok(data) => {
                            if let Ok(value) = serde_json::from_str::<T>(&data) {
                                // `set_always`, not `set`: every SSE event
                                // must notify even on an equal payload
                                // (see the socket comment).
                                incoming.set_always(Some(value));
                            }
                        }
                        Err(_) => {
                            status.set(SseStatus::Error);
                            return;
                        }
                    }
                }
                if alive.get() {
                    status.set(SseStatus::Closed);
                }
            }
            Err(_) => {
                if alive.get() {
                    status.set(SseStatus::Error);
                }
            }
        }
    });

    UseSse { incoming, status }
}
