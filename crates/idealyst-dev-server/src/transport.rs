//! WebSocket transport for the dev-side of the hot-reload protocol.
//!
//! Single-threaded blocking server: accept one client at a time,
//! drive its connection until disconnect, then accept the next. The
//! framework's reactive runtime lives in the same thread (it's
//! `Rc`-based, not `Send`), so we can't trivially go async without
//! moving the framework off `Rc`. Sync is fine for a dev-only tool
//! talking to a single connected device.
//!
//! Wire framing: each WebSocket message body is JSON-encoded
//! [`DevToApp`] (server → client) or [`AppToDev`] (client → server).
//! Swap for `postcard` or `bincode` later — `idealyst_wire::codec`
//! already provides the indirection; pointed at JSON for now to keep
//! the wire inspectable in network captures.

use std::net::{TcpListener, ToSocketAddrs};
use std::time::Duration;

use idealyst_wire::{AppToDev, DevToApp, WireColorScheme, WireTheme, PROTOCOL_VERSION};
use tungstenite::{Message, WebSocket};

use crate::WireRecordingBackend;

/// Listen on `addr` and serve dev-mode hot-reload connections from
/// the supplied recorder. Blocks forever; returns only on a fatal
/// socket error.
///
/// The recorder must already have been populated by an initial
/// `framework_core::render(...)` call before this is invoked. The
/// initial command batch (everything the recorder has captured up
/// to this point) is shipped to each new client on connect.
pub fn serve(addr: impl ToSocketAddrs, recorder: WireRecordingBackend) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    let local = listener.local_addr().ok();
    if let Some(addr) = local {
        eprintln!("[dev-server] listening on ws://{}", addr);
    }

    // Snapshot the initial-mount commands the recorder has buffered
    // by now (the caller has already run `framework_core::render(...)`
    // against it). Every new connection gets a fresh copy of this
    // snapshot — without it, the second client (e.g. after a browser
    // hard-reload) would see an empty buffer and a blank page.
    let initial_snapshot = recorder.drain_commands();
    eprintln!(
        "[dev-server] captured {} initial commands for replay",
        initial_snapshot.len()
    );

    loop {
        let (stream, peer) = listener.accept()?;
        eprintln!("[dev-server] client connected: {}", peer);
        if let Err(e) = handle_connection(stream, &recorder, &initial_snapshot) {
            eprintln!("[dev-server] client error: {}", e);
        }
        eprintln!("[dev-server] client disconnected");
    }
}

fn handle_connection(
    stream: std::net::TcpStream,
    recorder: &WireRecordingBackend,
    initial_snapshot: &[idealyst_wire::Command],
) -> Result<(), TransportError> {
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut ws = tungstenite::accept(stream).map_err(TransportError::Handshake)?;

    // Greet. Carries the protocol version + a placeholder theme so
    // the app can prepare its style state before the first apply.
    // `IDEALYST_REBUILT_AT_MS` is set by `watch::self_exec` before
    // restart so the new process can tell the app exactly when the
    // change was detected — letting the app log a real end-to-end
    // "change → apply" latency.
    let rebuilt_at_ms = std::env::var("IDEALYST_REBUILT_AT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok());
    let hello = DevToApp::Hello {
        protocol_version: PROTOCOL_VERSION,
        theme: WireTheme {
            name: "default".into(),
            color_scheme: WireColorScheme::Auto,
            tokens: Vec::new(),
        },
        rebuilt_at_ms,
    };
    send(&mut ws, &hello)?;

    // Ship the persistent initial-mount snapshot. A clone so
    // subsequent connections also see it.
    if !initial_snapshot.is_empty() {
        eprintln!(
            "[dev-server] sending {} initial commands",
            initial_snapshot.len()
        );
        send(&mut ws, &DevToApp::Commands(initial_snapshot.to_vec()))?;
    }
    // Also drain anything queued since the snapshot (e.g. events
    // from a previous connection that fired reactivity).
    let pending_since_snapshot = recorder.drain_commands();
    if !pending_since_snapshot.is_empty() {
        send(&mut ws, &DevToApp::Commands(pending_since_snapshot))?;
    }

    // Receive loop. Read with a short timeout so the loop can
    // periodically drain any commands the recorder may have queued
    // (e.g. from a timer effect on the dev side).
    loop {
        match ws.read() {
            Ok(Message::Text(t)) => {
                let msg: AppToDev = serde_json::from_str(t.as_str())
                    .map_err(|e| TransportError::Decode(e.to_string()))?;
                handle_app_msg(recorder, msg);
            }
            Ok(Message::Binary(b)) => {
                let msg: AppToDev = serde_json::from_slice(&b)
                    .map_err(|e| TransportError::Decode(e.to_string()))?;
                handle_app_msg(recorder, msg);
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(Message::Ping(p)) => {
                let _ = ws.send(Message::Pong(p));
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Timeout — flush any queued commands.
            }
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return Ok(());
            }
            Err(e) => return Err(TransportError::Tungstenite(e)),
        }

        // Flush any commands the recorder produced (from the app
        // event we just dispatched, or from a side effect).
        let pending = recorder.drain_commands();
        if !pending.is_empty() {
            send(&mut ws, &DevToApp::Commands(pending))?;
        }
    }
}

fn handle_app_msg(recorder: &WireRecordingBackend, msg: AppToDev) {
    match msg {
        AppToDev::Hello { app_name, color_scheme: _ } => {
            eprintln!("[dev-server] app hello: {}", app_name);
        }
        AppToDev::Event { handler, args } => {
            let _ = recorder.dispatch_event(handler, args);
        }
        AppToDev::StateChanged { node, bit, on } => {
            let _ = recorder.dispatch_state(node, bit, on);
        }
        AppToDev::ColorSchemeChanged { scheme: _ } => {
            // Theming hook: re-resolve stylesheets here in a follow-up.
        }
        AppToDev::ScreenReleased { scope } => {
            recorder.handle_screen_released(scope.0);
        }
        AppToDev::NavigatorDepthChanged { .. } => {
            // Dev tracks depth from its own stack model; informational.
        }
        AppToDev::DrawerStateChanged { navigator, is_open } => {
            recorder.handle_drawer_state_changed(navigator, is_open);
        }
        AppToDev::TabSelected { navigator, index } => {
            recorder.handle_tab_selected(navigator, index);
        }
        AppToDev::VirtualizerMountItem { .. }
        | AppToDev::VirtualizerReleaseItem { .. }
        | AppToDev::VirtualizerMeasuredSize { .. } => {
            // Virtualizer lazy-mount path — deferred (see
            // `create_virtualizer` doc in lib.rs for the plan).
        }
        AppToDev::Error { message } => {
            eprintln!("[dev-server] app reported error: {}", message);
        }
    }
}

fn send<S>(ws: &mut WebSocket<S>, msg: &DevToApp) -> Result<(), TransportError>
where
    S: std::io::Read + std::io::Write,
{
    let bytes = serde_json::to_vec(msg).map_err(|e| TransportError::Encode(e.to_string()))?;
    ws.send(Message::Binary(bytes.into()))
        .map_err(TransportError::Tungstenite)?;
    Ok(())
}

#[derive(Debug)]
pub enum TransportError {
    Io(std::io::Error),
    Handshake(tungstenite::handshake::HandshakeError<tungstenite::ServerHandshake<std::net::TcpStream, tungstenite::handshake::server::NoCallback>>),
    Tungstenite(tungstenite::Error),
    Encode(String),
    Decode(String),
}

impl From<std::io::Error> for TransportError {
    fn from(e: std::io::Error) -> Self {
        TransportError::Io(e)
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Io(e) => write!(f, "io: {}", e),
            TransportError::Handshake(_) => write!(f, "websocket handshake failed"),
            TransportError::Tungstenite(e) => write!(f, "websocket: {}", e),
            TransportError::Encode(s) => write!(f, "encode: {}", s),
            TransportError::Decode(s) => write!(f, "decode: {}", s),
        }
    }
}

impl std::error::Error for TransportError {}
