//! WebSocket transport for the dev-side of the hot-reload protocol.
//!
//! **Multi-client, single-threaded poll loop.** The framework runtime
//! is `Rc`-based (not `Send`), so we can't spawn threads that touch
//! it. Instead, [`serve`] runs a single thread that:
//!
//! 1. Accepts new TCP connections (non-blocking).
//! 2. Reads pending WebSocket frames from every connected client
//!    (non-blocking on each socket).
//! 3. Dispatches inbound app events through the recorder, which may
//!    fire reactive effects and append new commands to the log.
//! 4. Broadcasts any new commands to every connected client (each
//!    advances its own cursor independently).
//!
//! Net effect: an event from any client is visible to all clients
//! on the next tick. Live collaborative sessions fall out of the
//! AAS architecture for free — same logical app, multiple
//! interpreters.
//!
//! Wire framing: each WebSocket message body is JSON-encoded
//! [`DevToApp`] (server → client) or [`AppToDev`] (client → server).

use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

use wire::{AppToDev, DevToApp, WireColorScheme, WireTheme, PROTOCOL_VERSION};
use tungstenite::{Message, WebSocket};

use crate::WireRecordingBackend;

/// Tick between polls. Bounds the worst-case forwarding latency.
/// 20ms is well under human-perceptible delay for interaction
/// echo across clients while keeping CPU idle most of the time.
const TICK_INTERVAL: Duration = Duration::from_millis(20);

/// One connected client. Holds the WebSocket plus the per-client
/// cursor into the recorder's append-only log.
struct ClientConn {
    ws: WebSocket<TcpStream>,
    cursor: usize,
    /// Best-effort label for log lines.
    peer: String,
}

/// Listen on `addr` and serve dev-mode hot-reload connections from
/// the supplied recorder. Blocks forever; returns only on a fatal
/// socket error.
pub fn serve(addr: impl ToSocketAddrs, recorder: WireRecordingBackend) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    if let Ok(addr) = listener.local_addr() {
        eprintln!("[dev-server] listening on ws://{}", addr);
    }
    eprintln!(
        "[dev-server] recorder log starts with {} commands",
        recorder.command_count()
    );

    let mut clients: Vec<ClientConn> = Vec::new();

    loop {
        accept_new(&listener, &mut clients, &recorder);
        poll_reads(&mut clients, &recorder);
        broadcast_new_commands(&mut clients, &recorder);
        std::thread::sleep(TICK_INTERVAL);
    }
}

/// Accept any pending connections without blocking. Each accepted
/// client gets the protocol Hello + a one-shot catch-up batch
/// with the entire current log, then joins the broadcast set.
fn accept_new(
    listener: &TcpListener,
    clients: &mut Vec<ClientConn>,
    recorder: &WireRecordingBackend,
) {
    loop {
        match listener.accept() {
            Ok((stream, peer)) => {
                eprintln!(
                    "[dev-server] client connected: {} ({} active)",
                    peer,
                    clients.len() + 1
                );
                // tungstenite's handshake is synchronous; do it on a
                // briefly-blocking stream, then flip to non-blocking
                // for the steady-state poll loop.
                let _ = stream.set_nonblocking(false);
                let mut ws = match tungstenite::accept(stream) {
                    Ok(ws) => ws,
                    Err(e) => {
                        eprintln!("[dev-server] handshake failed: {}", e);
                        continue;
                    }
                };
                if send_hello(&mut ws).is_err() {
                    continue;
                }
                let catchup = recorder.commands_since(0);
                if !catchup.is_empty() {
                    eprintln!(
                        "[dev-server] catching up {} with {} commands",
                        peer,
                        catchup.len()
                    );
                    if send(&mut ws, &DevToApp::Commands(catchup)).is_err() {
                        continue;
                    }
                }
                let _ = ws.get_mut().set_nonblocking(true);
                clients.push(ClientConn {
                    ws,
                    cursor: recorder.command_count(),
                    peer: peer.to_string(),
                });
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => return,
            Err(e) => {
                eprintln!("[dev-server] accept error: {}", e);
                return;
            }
        }
    }
}

/// Drain any pending WebSocket frames from each connected client.
/// Disconnected / errored clients are removed.
fn poll_reads(clients: &mut Vec<ClientConn>, recorder: &WireRecordingBackend) {
    let mut keep = Vec::with_capacity(clients.len());
    for mut client in clients.drain(..) {
        let mut alive = true;
        // Drain as many frames as are immediately available.
        loop {
            match client.ws.read() {
                Ok(Message::Text(t)) => match serde_json::from_str::<AppToDev>(t.as_str()) {
                    Ok(msg) => handle_app_msg(recorder, msg),
                    Err(e) => eprintln!("[dev-server] decode error: {}", e),
                },
                Ok(Message::Binary(b)) => match serde_json::from_slice::<AppToDev>(&b) {
                    Ok(msg) => handle_app_msg(recorder, msg),
                    Err(e) => eprintln!("[dev-server] decode error: {}", e),
                },
                Ok(Message::Close(_)) => {
                    eprintln!("[dev-server] {} closed", client.peer);
                    alive = false;
                    break;
                }
                Ok(Message::Ping(p)) => {
                    let _ = client.ws.send(Message::Pong(p));
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => break,
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    eprintln!("[dev-server] {} disconnected", client.peer);
                    alive = false;
                    break;
                }
                Err(e) => {
                    eprintln!("[dev-server] {} read error: {}", client.peer, e);
                    alive = false;
                    break;
                }
            }
        }
        if alive {
            keep.push(client);
        }
    }
    *clients = keep;
}

/// Ship every command appended to the log since each client's
/// cursor. Disconnected clients are pruned.
fn broadcast_new_commands(
    clients: &mut Vec<ClientConn>,
    recorder: &WireRecordingBackend,
) {
    let new_count = recorder.command_count();
    let mut keep = Vec::with_capacity(clients.len());
    for mut client in clients.drain(..) {
        if new_count > client.cursor {
            let cmds = recorder.commands_since(client.cursor);
            if send(&mut client.ws, &DevToApp::Commands(cmds)).is_err() {
                eprintln!("[dev-server] {} send failed; dropping", client.peer);
                continue;
            }
            client.cursor = new_count;
        }
        keep.push(client);
    }
    *clients = keep;
}

fn send_hello<S>(ws: &mut WebSocket<S>) -> Result<(), TransportError>
where
    S: std::io::Read + std::io::Write,
{
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
    send(ws, &hello)
}

fn handle_app_msg(recorder: &WireRecordingBackend, msg: AppToDev) {
    match msg {
        AppToDev::Hello { app_name, color_scheme: _, initial_url: _ } => {
            eprintln!("[dev-server] app hello: {}", app_name);
        }
        AppToDev::Event { handler, args } => {
            let _ = recorder.dispatch_event(handler, args);
        }
        AppToDev::StateChanged { node, bit, on } => {
            let _ = recorder.dispatch_state(node, bit, on);
        }
        AppToDev::ColorSchemeChanged { scheme: _ } => {}
        AppToDev::ScreenReleased { scope } => {
            eprintln!("[transport] AppToDev::ScreenReleased(scope={})", scope.0);
            recorder.handle_screen_released(scope.0);
        }
        AppToDev::NavigatorDepthChanged { .. } => {}
        AppToDev::DrawerStateChanged { navigator, is_open } => {
            recorder.handle_drawer_state_changed(navigator, is_open);
        }
        AppToDev::TabSelected { navigator, index } => {
            recorder.handle_tab_selected(navigator, index);
        }
        AppToDev::VirtualizerMountItem { .. }
        | AppToDev::VirtualizerReleaseItem { .. }
        | AppToDev::VirtualizerMeasuredSize { .. } => {}
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
