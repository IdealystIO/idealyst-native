//! Test-support utilities for end-to-end multi-session tests.
//!
//! Provides:
//!
//! - [`MockClient`] — a WebSocket-based stand-in for a real platform
//!   client (web/iOS/Android). Performs the Hello exchange under a
//!   chosen identity, records every command the server broadcasts to
//!   it, and exposes assertion-friendly accessors so tests can pin
//!   isolation / fan-out behavior at the wire level.
//!
//! - [`emit_from_fake_sidecar`] — helper that shapes the
//!   `Sender<SidecarOut>` half of `Sidecar::for_test_with_channels`
//!   into a one-call "ship these commands as if produced by the
//!   author runtime for session X" API.
//!
//! Why a real socket and not an in-process bypass: the host
//! transport's correctness depends on the WebSocket lifecycle —
//! handshake, frame buffering, close semantics. An in-process bypass
//! would let routing tests pass while the real production
//! transport silently corrupted state. Using the actual loopback
//! socket keeps the test honest at the cost of one TCP connection
//! per `MockClient`.
//!
//! Always compiled — see the comment in `lib.rs` on the `pub mod
//! test_support` line for why this isn't feature-gated.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use tungstenite::{Message, WebSocket};
use wire::{AppToDev, ClientIdentity, Command, DevToApp, EventArgs, HandlerId};

use crate::SidecarOut;

/// One in-process mock dev client. Owns a real loopback WebSocket to
/// the dev-server under test; records every `DevToApp::Commands` batch
/// it receives so tests can assert which sessions saw what.
///
/// **Single-threaded.** All inbound frames are drained on demand via
/// [`Self::drain_for`] / [`Self::expect_at_least`] — the test owns the
/// schedule. This avoids the spawn-a-reader-thread complexity for the
/// modest cost of a busy-poll inside `drain_for`.
pub struct MockClient {
    ws: WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    /// Session id the server assigned, captured during
    /// [`Self::connect`]. Empty string if the server's Hello had no
    /// session (pre-v5 server — should never happen against current
    /// code).
    pub assigned_session: String,
    /// Protocol version the server reported, captured during
    /// [`Self::connect`].
    pub protocol_version: u32,
    /// Every command the server has broadcast to this client since
    /// connect, in receipt order.
    pub commands: Vec<Command>,
}

impl MockClient {
    /// Connect to the dev-server at `url`, send a Hello carrying the
    /// supplied identity, and synchronously read back the server's
    /// Hello frame so the assigned session id is available before any
    /// other test code runs.
    pub fn connect(url: &str, identity: ClientIdentity) -> Self {
        let (mut ws, _) = tungstenite::connect(url).expect("MockClient connect");
        if let tungstenite::stream::MaybeTlsStream::Plain(s) = ws.get_ref() {
            let _ = s.set_read_timeout(Some(Duration::from_millis(50)));
        }
        let hello = AppToDev::Hello {
            app_name: "mock-client".to_string(),
            color_scheme: wire::WireColorScheme::Auto,
            initial_url: None,
            identity,
            viewport: None,
            supports_screenshot: false,
        };
        ws.send(Message::Binary(
            serde_json::to_vec(&hello).expect("encode AppToDev::Hello").into(),
        ))
        .expect("send Hello");

        let (assigned_session, protocol_version) = read_dev_hello(&mut ws);
        Self {
            ws,
            assigned_session,
            protocol_version,
            commands: Vec::new(),
        }
    }

    /// Drain inbound frames for `duration`, appending every command in
    /// every `Commands` batch to [`Self::commands`]. Returns the
    /// number of *new* commands captured during this call.
    ///
    /// Use this when you've kicked off some server work (e.g. emitted
    /// commands from the fake sidecar) and want to give the host's
    /// 20ms broadcast tick enough time to deliver them.
    pub fn drain_for(&mut self, duration: Duration) -> usize {
        let deadline = Instant::now() + duration;
        let start_len = self.commands.len();
        while Instant::now() < deadline {
            match self.ws.read() {
                Ok(Message::Binary(b)) => {
                    self.handle_dev_msg(&b);
                }
                Ok(Message::Text(t)) => {
                    self.handle_dev_msg(t.as_bytes());
                }
                Ok(Message::Ping(p)) => {
                    let _ = self.ws.send(Message::Pong(p));
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => {
                    panic!(
                        "MockClient {:?} read error: {}",
                        self.assigned_session, e
                    );
                }
            }
        }
        self.commands.len() - start_len
    }

    /// Convenience: drain until at least `expected` commands have
    /// been seen, panicking after `timeout` if not. Returns a
    /// reference to the full received-so-far buffer.
    pub fn expect_at_least(&mut self, expected: usize, timeout: Duration) -> &[Command] {
        let deadline = Instant::now() + timeout;
        while self.commands.len() < expected && Instant::now() < deadline {
            self.drain_for(Duration::from_millis(50));
        }
        assert!(
            self.commands.len() >= expected,
            "MockClient {:?}: expected at least {} commands within {:?}, got {} (received so far: {:?})",
            self.assigned_session,
            expected,
            timeout,
            self.commands.len(),
            self.commands,
        );
        &self.commands
    }

    /// Send a fake event back at the server. The server will look up
    /// this client's session and forward to the (fake) sidecar.
    pub fn send_event(&mut self, handler: HandlerId, args: EventArgs) {
        let event = AppToDev::Event { handler, args };
        self.ws
            .send(Message::Binary(
                serde_json::to_vec(&event).unwrap().into(),
            ))
            .expect("MockClient send_event");
    }

    /// Gracefully close the socket.
    pub fn close(mut self) {
        let _ = self.ws.close(None);
    }

    fn handle_dev_msg(&mut self, bytes: &[u8]) {
        let msg: DevToApp = match serde_json::from_slice(bytes) {
            Ok(m) => m,
            Err(e) => panic!(
                "MockClient {:?}: decode error: {}",
                self.assigned_session, e
            ),
        };
        match msg {
            DevToApp::Hello { .. } => {
                // Server re-sent Hello? Shouldn't happen — connect()
                // already consumed the first one. Ignore.
            }
            DevToApp::Commands(cmds) => {
                self.commands.extend(cmds);
            }
            DevToApp::Rebuilding => {}
            DevToApp::Error { message } => panic!(
                "MockClient {:?}: server reported error: {}",
                self.assigned_session, message
            ),
            DevToApp::ThemeChanged { .. } => {}
            DevToApp::CaptureScreenshot { .. } => {
                // MockClient doesn't render a real surface; ignore.
            }
        }
    }
}

fn read_dev_hello(
    ws: &mut WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
) -> (String, u32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match ws.read() {
            Ok(Message::Binary(b)) => return parse_hello(&b),
            Ok(Message::Text(t)) => return parse_hello(t.as_bytes()),
            Ok(Message::Ping(p)) => {
                let _ = ws.send(Message::Pong(p));
            }
            Ok(Message::Close(_)) => panic!("server closed before Hello"),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => panic!("MockClient: error reading server Hello: {}", e),
        }
    }
    panic!("MockClient: server Hello never arrived");
}

fn parse_hello(bytes: &[u8]) -> (String, u32) {
    match serde_json::from_slice::<DevToApp>(bytes).expect("decode DevToApp Hello") {
        DevToApp::Hello {
            protocol_version,
            session,
            ..
        } => (session, protocol_version),
        other => panic!("expected DevToApp::Hello, got {:?}", other),
    }
}

/// Push a `Commands` batch onto the fake-sidecar→host channel as if
/// the named session's author runtime had emitted them. The host's
/// `drain_sidecar_inbound` picks them up on the next tick (≤20ms) and
/// broadcasts to the session's attached clients.
pub fn emit_from_fake_sidecar(
    fake_out_tx: &mpsc::Sender<SidecarOut>,
    session: &str,
    cmds: Vec<Command>,
) {
    fake_out_tx
        .send(SidecarOut::Commands {
            session: session.to_string(),
            cmds,
        })
        .expect("fake sidecar send Commands");
}

/// Same as [`emit_from_fake_sidecar`] but emits a `SessionReset`
/// frame first — simulates the post-hot-patch flow where the
/// session's author runtime tears down + re-renders, and the host
/// needs to drop its mirror's log before applying the new snapshot.
pub fn emit_session_reset_then(
    fake_out_tx: &mpsc::Sender<SidecarOut>,
    session: &str,
    cmds: Vec<Command>,
) {
    fake_out_tx
        .send(SidecarOut::SessionReset {
            session: session.to_string(),
        })
        .expect("fake sidecar send SessionReset");
    emit_from_fake_sidecar(fake_out_tx, session, cmds);
}
