//! WebSocket transport for the app side of the hot-reload protocol.
//!
//! Single-threaded blocking client: connects to the dev server,
//! reads incoming [`DevToApp`] messages, applies them to the
//! [`WireBackend`], and forwards outbound [`AppToDev`] messages from
//! the backend's event channel back over the socket.
//!
//! This is meant to drive the app's main thread when the host opts
//! into the `dev-hot-reload` feature. The blocking loop is fine on a
//! dedicated dev-only entry point; production binaries don't
//! compile this in.

use std::io::ErrorKind;
use std::sync::mpsc;
use std::time::Duration;

use framework_core::Backend;
use idealyst_wire::{AppToDev, DevToApp};
use tungstenite::{Message, WebSocket};

use crate::WireBackend;

/// Connect to the dev server at `url` (e.g. `ws://127.0.0.1:9001`),
/// hand the supplied `WireBackend` everything we receive, and ship
/// outbound events from the backend's channel back to the dev side.
///
/// Blocks until the connection closes. Caller controls reconnection
/// strategy.
pub fn connect_and_run<B: Backend + 'static>(
    url: &str,
    wire: &mut WireBackend<B>,
    outbound_rx: mpsc::Receiver<AppToDev>,
) -> Result<(), ClientError>
where
    B::Node: 'static,
{
    let (mut ws, _) = tungstenite::connect(url).map_err(ClientError::Connect)?;

    // Short read timeout lets the loop also poll the outbound queue.
    if let Some(stream) = underlying_stream(&ws) {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    }

    // Greet.
    let hello = AppToDev::Hello {
        app_name: env!("CARGO_PKG_NAME").to_string(),
        color_scheme: crate::color_scheme_to_wire(wire.color_scheme()),
        // Native transport — no URL bar to read from.
        initial_url: None,
    };
    send(&mut ws, &hello)?;

    loop {
        match ws.read() {
            Ok(Message::Text(t)) => {
                let msg: DevToApp = serde_json::from_str(t.as_str())
                    .map_err(|e| ClientError::Decode(e.to_string()))?;
                apply_dev_msg(wire, msg)?;
            }
            Ok(Message::Binary(b)) => {
                let msg: DevToApp = serde_json::from_slice(&b)
                    .map_err(|e| ClientError::Decode(e.to_string()))?;
                apply_dev_msg(wire, msg)?;
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(Message::Ping(p)) => {
                let _ = ws.send(Message::Pong(p));
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                // Read timeout — fall through to drain outbound.
            }
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return Ok(());
            }
            Err(e) => return Err(ClientError::Tungstenite(e)),
        }

        // Drain any pending app→dev events. Non-blocking; loops
        // until empty so a burst of events all flush at once.
        loop {
            match outbound_rx.try_recv() {
                Ok(msg) => send_app(&mut ws, &msg)?,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }
}

fn apply_dev_msg<B: Backend + 'static>(
    wire: &mut WireBackend<B>,
    msg: DevToApp,
) -> Result<(), ClientError>
where
    B::Node: 'static,
{
    match msg {
        DevToApp::Hello { protocol_version, .. } => {
            if protocol_version != idealyst_wire::PROTOCOL_VERSION {
                eprintln!(
                    "[dev-client] protocol version mismatch: dev={}, app={}",
                    protocol_version,
                    idealyst_wire::PROTOCOL_VERSION
                );
            }
            // Theme application is a follow-up (needs to feed into
            // backend.install_theme_variables once the wire form is
            // populated by the dev side).
        }
        DevToApp::Commands(cmds) => {
            wire.apply_batch(cmds).map_err(|e| ClientError::Replay(format!("{:?}", e)))?;
        }
        DevToApp::Rebuilding => {
            eprintln!("[dev-client] dev is rebuilding…");
        }
        DevToApp::Error { message } => {
            eprintln!("[dev-client] dev error: {}", message);
        }
        DevToApp::ThemeChanged { .. } => {
            // See Hello comment.
        }
    }
    Ok(())
}

fn send<S>(ws: &mut WebSocket<S>, msg: &AppToDev) -> Result<(), ClientError>
where
    S: std::io::Read + std::io::Write,
{
    let bytes = serde_json::to_vec(msg).map_err(|e| ClientError::Encode(e.to_string()))?;
    ws.send(Message::Binary(bytes.into()))
        .map_err(ClientError::Tungstenite)?;
    Ok(())
}

fn send_app<S>(ws: &mut WebSocket<S>, msg: &AppToDev) -> Result<(), ClientError>
where
    S: std::io::Read + std::io::Write,
{
    send(ws, msg)
}

/// Reach the underlying `TcpStream` from a tungstenite WebSocket so
/// we can configure socket-level options (read timeout).
fn underlying_stream(
    ws: &tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
) -> Option<&std::net::TcpStream> {
    match ws.get_ref() {
        tungstenite::stream::MaybeTlsStream::Plain(s) => Some(s),
        _ => None,
    }
}

#[derive(Debug)]
pub enum ClientError {
    Connect(tungstenite::Error),
    Tungstenite(tungstenite::Error),
    Encode(String),
    Decode(String),
    Replay(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Connect(e) => write!(f, "connect failed: {}", e),
            ClientError::Tungstenite(e) => write!(f, "websocket: {}", e),
            ClientError::Encode(s) => write!(f, "encode: {}", s),
            ClientError::Decode(s) => write!(f, "decode: {}", s),
            ClientError::Replay(s) => write!(f, "replay: {}", s),
        }
    }
}

impl std::error::Error for ClientError {}
