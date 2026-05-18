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

use mdns_sd::{ServiceDaemon, ServiceInfo};
use tungstenite::{Message, WebSocket};
use wire::{AppToDev, DevToApp, WireColorScheme, WireTheme, PROTOCOL_VERSION};

use crate::WireRecordingBackend;

/// DNS-SD service type. Clients (iOS/web/etc.) browse for this and
/// filter the matched services' TXT records by `app_id` to find the
/// right server.
pub const SERVICE_TYPE: &str = "_idealyst-dev._tcp.local.";

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
///
/// `app_id` is what clients filter on via the mDNS TXT record to
/// find the right server. Multiple dev-servers can run on the same
/// machine — each advertises with its own id and ephemeral port.
///
/// Pass `addr` of `"0.0.0.0:0"` to let the OS assign a port (and
/// listen on every interface, so a phone on LAN can connect). The
/// actual bound port goes into the mDNS advertisement, so clients
/// don't need to know it ahead of time.
pub fn serve(
    addr: impl ToSocketAddrs,
    recorder: WireRecordingBackend,
    app_id: &str,
) -> std::io::Result<()> {
    serve_with_tick_and_port(addr, recorder, app_id, || {}, None)
}

/// Same as [`serve`] but writes the bound port into the supplied
/// `Arc<Mutex<Option<u16>>>` right after `TcpListener::bind`. Used by
/// the AAS host wrapper to thread the port through the rebuild
/// loop's `before_exec` hook — that way the next process image
/// rebinds the same port via `IDEALYST_AAS_BIND_PORT`, so any
/// `adb reverse` tunnels (and any hard-coded URLs) stay valid
/// across a hot reload.
pub fn serve_with_port_mirror(
    addr: impl ToSocketAddrs,
    recorder: WireRecordingBackend,
    app_id: &str,
    port_mirror: std::sync::Arc<std::sync::Mutex<Option<u16>>>,
) -> std::io::Result<()> {
    serve_with_tick_and_port(addr, recorder, app_id, || {}, Some(port_mirror))
}

/// Like [`serve`] but runs `on_tick` once per loop iteration on the
/// server thread (the same thread that owns the walker's reactive
/// scope). Used by [`serve_with_robot_bridge`] to drive the Robot
/// bridge poll; other consumers can use it for any per-tick work
/// that needs to run on the reactive thread.
///
/// `TICK_INTERVAL` (currently 16ms) caps the per-tick rate — busy
/// callbacks here directly delay accept / read / broadcast work, so
/// keep them light.
pub fn serve_with_tick<F>(
    addr: impl ToSocketAddrs,
    recorder: WireRecordingBackend,
    app_id: &str,
    on_tick: F,
) -> std::io::Result<()>
where
    F: FnMut(),
{
    serve_with_tick_and_port(addr, recorder, app_id, on_tick, None)
}

/// Internal workhorse. Bundles the optional `port_mirror` (used by
/// the AAS host to persist its bound port across self-exec) into the
/// otherwise unchanged `serve_with_tick` body. Public callers go
/// through [`serve`], [`serve_with_tick`], or
/// [`serve_with_port_mirror`].
fn serve_with_tick_and_port<F>(
    addr: impl ToSocketAddrs,
    recorder: WireRecordingBackend,
    app_id: &str,
    mut on_tick: F,
    port_mirror: Option<std::sync::Arc<std::sync::Mutex<Option<u16>>>>,
) -> std::io::Result<()>
where
    F: FnMut(),
{
    let listener = TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    let bound = listener.local_addr().ok();
    if let Some(a) = bound {
        eprintln!("[dev-server] listening on ws://{}", a);
        if let Some(m) = &port_mirror {
            if let Ok(mut g) = m.lock() {
                *g = Some(a.port());
            }
        }
    }
    eprintln!(
        "[dev-server] recorder log starts with {} commands",
        recorder.command_count()
    );

    // Hold the mDNS handle for the life of the server. Drop unregisters.
    let _mdns = match bound {
        Some(a) => match advertise_mdns(app_id, a.port()) {
            Ok(h) => Some(h),
            Err(e) => {
                eprintln!(
                    "[dev-server] mDNS advertise failed (clients won't auto-discover): {}",
                    e
                );
                None
            }
        },
        None => None,
    };

    let mut clients: Vec<ClientConn> = Vec::new();

    loop {
        accept_new(&listener, &mut clients, &recorder);
        poll_reads(&mut clients, &recorder);
        broadcast_new_commands(&mut clients, &recorder);
        on_tick();
        std::thread::sleep(TICK_INTERVAL);
    }
}

/// Like [`serve`] but also drives a Robot bridge handle once per
/// tick, on this thread (same thread that owns the registry the
/// walker populated). Used in AAS mode where the framework runs on
/// the dev-server, so robot commands from an external MCP proxy
/// must hit the server's registry — the AAS client has none.
///
/// Native (non-AAS) app builds put the bridge in-process on the
/// device; this function is the server-side analogue for AAS
/// deployments, regardless of which platform is hosting the AAS
/// client.
#[cfg(feature = "robot")]
pub fn serve_with_robot_bridge(
    addr: impl ToSocketAddrs,
    recorder: WireRecordingBackend,
    app_id: &str,
    bridge: framework_core::robot::bridge::BridgeHandle,
) -> std::io::Result<()> {
    serve_with_tick(addr, recorder, app_id, move || bridge.poll())
}

/// RAII wrapper around the mDNS service registration so the
/// advertisement goes away when `serve` returns.
struct MdnsHandle {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Drop for MdnsHandle {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        // shutdown() returns a receiver; we don't wait — drop is
        // best-effort in dev tooling.
        let _ = self.daemon.shutdown();
    }
}

/// Spin up an mDNS daemon and publish the dev-server's service
/// record. Returns a handle whose drop unregisters the service.
///
/// We use `enable_addr_auto()` so the daemon advertises every
/// non-loopback interface the host has — that's what makes a
/// phone on LAN able to find the server while still working when
/// the simulator connects via loopback. Instance name is keyed
/// off `app_id` + pid so multiple concurrent dev-servers don't
/// shadow each other in the registry.
fn advertise_mdns(app_id: &str, port: u16) -> Result<MdnsHandle, Box<dyn std::error::Error>> {
    let daemon = ServiceDaemon::new()?;
    let pid = std::process::id();
    // DNS-SD hostnames live under `.local.` and conventionally use a
    // single label (letters, digits, hyphens). Reverse-DNS app ids
    // like `ai.truday.idealyst.docs` would produce a multi-label
    // hostname (`idealyst-ai.truday.idealyst.docs-<pid>.local.`),
    // which mDNS implementations silently refuse to publish — the
    // registration "succeeds" but the service never appears in
    // browses. Substitute dots → hyphens for the structural fields
    // and keep the original app id in the TXT record so clients
    // (which match on TXT, not the hostname) work unchanged.
    let app_id_label = app_id.replace('.', "-");
    let instance_name = format!("{}-{}", app_id_label, pid);
    let hostname = format!("idealyst-{}-{}.local.", app_id_label, pid);
    let proto = PROTOCOL_VERSION.to_string();
    let txt: [(&str, &str); 2] = [("app_id", app_id), ("proto", proto.as_str())];
    let info = ServiceInfo::new(SERVICE_TYPE, &instance_name, &hostname, "", port, &txt[..])?
        .enable_addr_auto();
    let fullname = info.get_fullname().to_string();
    daemon.register(info)?;
    eprintln!(
        "[dev-server] advertised via mDNS as {} ({}:{} on this host)",
        fullname, hostname, port
    );
    Ok(MdnsHandle { daemon, fullname })
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
                // Snapshot the live scene at the moment we accept,
                // and capture the cursor at the same instant. The
                // snapshot describes the *current* state (no
                // historical Push/Pop pairs that already cancelled
                // out), and the cursor anchors where this client's
                // incremental updates should resume.
                let cursor_at_snapshot = recorder.command_count();
                let snapshot = recorder.snapshot();
                if !snapshot.is_empty() {
                    eprintln!(
                        "[dev-server] catching up {} with snapshot of {} commands (log size {})",
                        peer,
                        snapshot.len(),
                        cursor_at_snapshot
                    );
                    if send(&mut ws, &DevToApp::Commands(snapshot)).is_err() {
                        continue;
                    }
                }
                let _ = ws.get_mut().set_nonblocking(true);
                clients.push(ClientConn {
                    ws,
                    cursor: cursor_at_snapshot,
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
