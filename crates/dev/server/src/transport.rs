//! WebSocket transport for the dev-side of the hot-reload protocol.
//!
//! **Session assignment is a server-side decision.** Clients never
//! name a session. The host is configured with a [`SessionMode`]:
//!
//! - [`SessionMode::PerClient`] (default): every connection gets a
//!   fresh server-minted id and its own isolated author runtime in
//!   the sidecar. Two phones connecting see independent scenes.
//! - [`SessionMode::Shared`]: every connection lands on the same
//!   well-known "shared" session. The legacy "one author, many synced
//!   devices" mode — flip the mode flag on the host (typically via
//!   `IDEALYST_AAS_MULTI_SESSION=0`) when you want all your devices
//!   to mirror each other.
//!
//! Clients still send a [`wire::ClientIdentity`] in their Hello so the
//! server's logs (and the future session-picker dev tool) can show
//! "iPhone 15 Pro Sim" rather than "ws://1.2.3.4:5678/anon".
//!
//! The host owns a per-session [`WireRecordingBackend`] *mirror* that
//! accumulates everything the sidecar emits for that session. New
//! clients catching up to an existing session get a snapshot off the
//! right mirror; live clients get incremental deltas keyed by per-client
//! cursors. A hot-patch rerender comes through as a
//! [`SidecarOut::SessionReset`] which clears the mirror and bumps its
//! epoch, forcing every client in that session to re-snapshot.
//!
//! **Single-threaded poll loop.** The framework runtime is `Rc`-based
//! and the per-session mirrors are `!Send`, so the entire transport
//! lives on one thread that:
//!
//! 1. Accepts new TCP connections (non-blocking).
//! 2. Reads pending WebSocket frames from every connected client.
//! 3. Dispatches inbound app events through the sidecar — tagged with
//!    the client's session id.
//! 4. Drains sidecar→host frames (`Commands`, `SessionReset`,
//!    `SessionEnded`, …) and applies them to the right session mirror.
//! 5. Broadcasts any new commands per session to the clients attached
//!    to that session (each advances its own cursor independently).
//!
//! Wire framing: each WebSocket message body is JSON-encoded
//! [`DevToApp`] (server → client) or [`AppToDev`] (client → server).

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceInfo};
use tungstenite::{Message, WebSocket};
use framework_core::ColorScheme;
use wire::{AppToDev, ClientIdentity, DevToApp, WireColorScheme, WireTheme, PROTOCOL_VERSION};

/// How the host assigns sessions to incoming clients. Controls the
/// "per-device" vs "synced collaborative" behavior end-to-end.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SessionMode {
    /// Default. Every connection gets a unique server-minted session
    /// id and its own isolated author runtime in the sidecar.
    PerClient,
    /// Legacy mode. Every connection attaches to one shared session
    /// (id: `"shared"`), so multiple devices drive the same scene.
    Shared,
}

impl Default for SessionMode {
    fn default() -> Self {
        Self::PerClient
    }
}

impl SessionMode {
    /// Parse from the `IDEALYST_AAS_MULTI_SESSION` env-var convention.
    /// `"0"` / `"false"` / `"no"` / `"off"` → `Shared`; anything else
    /// (including unset, empty, `"1"`) → `PerClient`.
    pub fn from_env() -> Self {
        match std::env::var("IDEALYST_AAS_MULTI_SESSION").ok().as_deref() {
            Some("0") | Some("false") | Some("no") | Some("off") | Some("FALSE") => {
                SessionMode::Shared
            }
            _ => SessionMode::PerClient,
        }
    }
}

fn wire_color_scheme_to_core(s: WireColorScheme) -> ColorScheme {
    match s {
        WireColorScheme::Light => ColorScheme::Light,
        WireColorScheme::Dark => ColorScheme::Dark,
        WireColorScheme::Auto => ColorScheme::Auto,
    }
}

use crate::WireRecordingBackend;

/// DNS-SD service type. Clients (iOS/web/etc.) browse for this and
/// filter the matched services' TXT records by `app_id` to find the
/// right server.
pub const SERVICE_TYPE: &str = "_idealyst-dev._tcp.local.";

/// Tick between polls. Bounds the worst-case forwarding latency.
/// 20ms is well under human-perceptible delay for interaction
/// echo across clients while keeping CPU idle most of the time.
const TICK_INTERVAL: Duration = Duration::from_millis(20);

/// Per-handshake budget the server waits for a client's first
/// [`AppToDev::Hello`]. Older client builds connect-then-listen so a
/// short timeout is fine; if no Hello arrives we close the socket.
const HANDSHAKE_DEADLINE: Duration = Duration::from_millis(2_000);

/// One connected client. Holds the WebSocket plus the per-client
/// cursor into the session mirror's append-only log.
struct ClientConn {
    ws: WebSocket<TcpStream>,
    cursor: usize,
    /// Mirror scene epoch this client is in sync with. When the
    /// session's mirror resets (after a hot-patch rerender), its epoch
    /// moves forward; the next broadcast tick detects the mismatch and
    /// sends the client a fresh `snapshot()` instead of the
    /// delta-from-cursor — which would be meaningless across a log
    /// truncation.
    epoch: u64,
    /// Best-effort label for log lines.
    peer: String,
    /// Session id this client is attached to. Indexes into
    /// `SessionTable`.
    session: String,
}

/// Host-side per-session state. The mirror accumulates everything the
/// sidecar emits for this session; a fresh client connecting to the
/// session gets its current snapshot.
struct SessionState {
    mirror: WireRecordingBackend,
}

impl SessionState {
    fn new() -> Self {
        Self {
            mirror: WireRecordingBackend::new(),
        }
    }
}

/// Map of live sessions, keyed by id.
struct SessionTable {
    sessions: HashMap<String, SessionState>,
    next_anonymous_seq: u64,
}

impl SessionTable {
    fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            next_anonymous_seq: 0,
        }
    }

    /// Mint a fresh server-side session id. The platform shows up in
    /// the id so logs and the future dev tool can tell sessions apart
    /// at a glance (e.g. `web_00000003`, `ios_00000001`). The numeric
    /// suffix is global, not per-platform, so collisions across kinds
    /// can't happen.
    fn mint_anonymous(&mut self, identity: &ClientIdentity) -> String {
        self.next_anonymous_seq += 1;
        let prefix = match identity.platform {
            wire::WirePlatform::Web => "web",
            wire::WirePlatform::Ios => "ios",
            wire::WirePlatform::Android => "android",
            wire::WirePlatform::MacOs => "macos",
            wire::WirePlatform::Linux => "linux",
            wire::WirePlatform::Windows => "windows",
            wire::WirePlatform::Other => "client",
        };
        format!("{}_{:08x}", prefix, self.next_anonymous_seq)
    }

    /// Look up `id`, creating a new entry under that id if absent.
    /// Returns `(state_ref, was_created)`. `was_created` tells the
    /// caller whether to send a `CreateSession` IPC frame to the
    /// sidecar.
    fn get_or_create(&mut self, id: &str) -> (&mut SessionState, bool) {
        let created = !self.sessions.contains_key(id);
        let state = self
            .sessions
            .entry(id.to_string())
            .or_insert_with(SessionState::new);
        (state, created)
    }

    fn get(&self, id: &str) -> Option<&SessionState> {
        self.sessions.get(id)
    }
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
///
/// `recorder` is retained as a "primary" mirror for single-process
/// (no-sidecar) mode — that's the only path that runs the user code
/// locally and emits commands directly into a host-owned recorder. In
/// sidecar mode the host's recorder is unused; per-session mirrors
/// take over.
pub fn serve(
    addr: impl ToSocketAddrs,
    recorder: WireRecordingBackend,
    app_id: &str,
) -> std::io::Result<()> {
    serve_with_tick_and_port(addr, recorder, app_id, || {}, None, None, None)
}

/// Default `SessionMode` for the legacy wrappers (`serve`,
/// `serve_with_port_mirror`, `serve_with_tick`, `serve_with_sidecar`)
/// — those callers don't take a mode parameter; they get
/// `SessionMode::PerClient`. Use [`serve_with_sidecar_and_tracker`]
/// if you need to override.
const LEGACY_DEFAULT_MODE: SessionMode = SessionMode::PerClient;

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
    serve_with_tick_and_port(addr, recorder, app_id, || {}, Some(port_mirror), None, None)
}

/// Same as [`serve_with_port_mirror`] but also forwards every
/// inbound `AppToDev` event to the sidecar held in `sidecar_slot`
/// — tagged with the connecting client's session id. Used by the
/// split-process AAS host where the user's reactive runtime lives
/// in the sidecar.
pub fn serve_with_sidecar(
    addr: impl ToSocketAddrs,
    recorder: WireRecordingBackend,
    app_id: &str,
    port_mirror: std::sync::Arc<std::sync::Mutex<Option<u16>>>,
    sidecar_slot: crate::SidecarSlot,
) -> std::io::Result<()> {
    serve_with_tick_and_port(
        addr,
        recorder,
        app_id,
        || {},
        Some(port_mirror),
        Some(sidecar_slot),
        None,
    )
}

/// Same as [`serve_with_sidecar`] but threads a [`crate::SessionTracker`]
/// the serve loop maintains in lock-step with its own session table.
/// The respawn / watcher thread reads the tracker after spawning a
/// fresh sidecar so it can replay `CreateSession` for every live
/// session, restoring per-session runtimes that would otherwise be
/// stranded by the respawn.
///
/// `mode` chooses between per-client and shared session assignment —
/// see [`SessionMode`].
pub fn serve_with_sidecar_and_tracker(
    addr: impl ToSocketAddrs,
    recorder: WireRecordingBackend,
    app_id: &str,
    port_mirror: std::sync::Arc<std::sync::Mutex<Option<u16>>>,
    sidecar_slot: crate::SidecarSlot,
    tracker: crate::SessionTracker,
    mode: SessionMode,
) -> std::io::Result<()> {
    serve_with_tick_and_port_and_mode(
        addr,
        recorder,
        app_id,
        || {},
        Some(port_mirror),
        Some(sidecar_slot),
        Some(tracker),
        mode,
    )
}

/// Like [`serve`] but runs `on_tick` once per loop iteration on the
/// server thread.
pub fn serve_with_tick<F>(
    addr: impl ToSocketAddrs,
    recorder: WireRecordingBackend,
    app_id: &str,
    on_tick: F,
) -> std::io::Result<()>
where
    F: FnMut(),
{
    serve_with_tick_and_port(addr, recorder, app_id, on_tick, None, None, None)
}

/// Public entry. Forwards to [`serve_with_tick_and_port_and_mode`]
/// using the default [`SessionMode::PerClient`]. New code should
/// prefer the explicit-mode variant.
pub fn serve_with_tick_and_port<F>(
    addr: impl ToSocketAddrs,
    recorder: WireRecordingBackend,
    app_id: &str,
    on_tick: F,
    port_mirror: Option<std::sync::Arc<std::sync::Mutex<Option<u16>>>>,
    sidecar_slot: Option<crate::SidecarSlot>,
    session_tracker: Option<crate::SessionTracker>,
) -> std::io::Result<()>
where
    F: FnMut(),
{
    serve_with_tick_and_port_and_mode(
        addr,
        recorder,
        app_id,
        on_tick,
        port_mirror,
        sidecar_slot,
        session_tracker,
        LEGACY_DEFAULT_MODE,
    )
}

/// Internal workhorse with explicit [`SessionMode`]. Everything else
/// (the no-mode public wrappers above) routes through here.
pub fn serve_with_tick_and_port_and_mode<F>(
    addr: impl ToSocketAddrs,
    recorder: WireRecordingBackend,
    app_id: &str,
    mut on_tick: F,
    port_mirror: Option<std::sync::Arc<std::sync::Mutex<Option<u16>>>>,
    sidecar_slot: Option<crate::SidecarSlot>,
    session_tracker: Option<crate::SessionTracker>,
    mode: SessionMode,
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
    let mut sessions = SessionTable::new();

    // Two pre-existing pinned sessions:
    //
    // - In single-process (no-sidecar) mode the host's recorder *is*
    //   the user runtime, so we register it under `"primary"` and pin
    //   every connecting client to it regardless of mode.
    // - In sidecar + `SessionMode::Shared` mode we register an empty
    //   `"shared"` mirror up front so every client lands on it. The
    //   sidecar gets one `CreateSession` for that id on the first
    //   client connect (same path as any other newly-created session).
    let single_process_mode = sidecar_slot.is_none();
    let pinned_session_id: Option<String> = if single_process_mode {
        sessions.sessions.insert(
            "primary".to_string(),
            SessionState {
                mirror: recorder.clone(),
            },
        );
        eprintln!(
            "[dev-server] single-process mode: primary session bound to host recorder ({} commands cached)",
            recorder.command_count(),
        );
        Some("primary".to_string())
    } else {
        match mode {
            SessionMode::PerClient => {
                eprintln!(
                    "[dev-server] sidecar mode: per-client sessions (each connection gets its own scene)"
                );
                None
            }
            SessionMode::Shared => {
                eprintln!(
                    "[dev-server] sidecar mode: shared session (every client drives the same scene)"
                );
                Some("shared".to_string())
            }
        }
    };

    loop {
        accept_new(
            &listener,
            &mut clients,
            &mut sessions,
            sidecar_slot.as_ref(),
            single_process_mode,
            pinned_session_id.as_deref(),
            session_tracker.as_ref(),
        );
        poll_reads(
            &mut clients,
            sidecar_slot.as_ref(),
            single_process_mode,
            &sessions,
            session_tracker.as_ref(),
        );
        drain_sidecar_inbound(
            &mut sessions,
            &mut clients,
            sidecar_slot.as_ref(),
            session_tracker.as_ref(),
        );
        broadcast_new_commands(&mut clients, &sessions);
        on_tick();
        std::thread::sleep(TICK_INTERVAL);
    }
}

/// Pull any pending sidecar→host frames and route them through the
/// per-session mirrors. Runs on the same thread as the rest of the
/// serve loop because `WireRecordingBackend` is `!Send`.
fn drain_sidecar_inbound(
    sessions: &mut SessionTable,
    clients: &mut Vec<ClientConn>,
    sidecar_slot: Option<&crate::SidecarSlot>,
    tracker: Option<&crate::SessionTracker>,
) {
    let Some(slot) = sidecar_slot else { return };
    let Ok(guard) = slot.lock() else { return };
    let Some(sidecar) = guard.as_ref() else { return };
    for msg in sidecar.drain_inbound() {
        match msg {
            crate::SidecarOut::Hello { aslr_reference } => {
                sidecar.set_aslr_reference(aslr_reference);
            }
            crate::SidecarOut::Commands { session, cmds } => {
                let Some(state) = sessions.sessions.get(&session) else {
                    eprintln!(
                        "[dev-server] sidecar Commands for unknown session {:?}; dropping {} cmds",
                        session,
                        cmds.len(),
                    );
                    continue;
                };
                for cmd in cmds {
                    state.mirror.push_external_command(cmd);
                }
            }
            crate::SidecarOut::SessionReady { session } => {
                eprintln!("[dev-server] session ready: {}", session);
            }
            crate::SidecarOut::SessionReset { session } => {
                // The sidecar's session thread tore down + re-rendered
                // after a hot-patch. Drop our mirror's log + scene so
                // the next `Commands` batch lands on an empty mirror,
                // and bump the epoch so every attached client gets
                // re-snapshotted on the next broadcast tick.
                if let Some(state) = sessions.sessions.get(&session) {
                    state.mirror.reset_log_and_scene();
                } else {
                    eprintln!(
                        "[dev-server] SessionReset for unknown session {:?}; ignoring",
                        session
                    );
                }
            }
            crate::SidecarOut::SessionEnded { session } => {
                eprintln!("[dev-server] session ended: {}", session);
                sessions.sessions.remove(&session);
                if let Some(t) = tracker {
                    t.remove(&session);
                }
                // Close every client that was attached to it. They'll
                // reconnect and (typically) land on a fresh session.
                clients.retain_mut(|c| {
                    if c.session == session {
                        let _ = c.ws.close(None);
                        false
                    } else {
                        true
                    }
                });
            }
        }
    }
}

/// Accept any pending connections without blocking. Each accepted
/// client gets one synchronous read budget for its `AppToDev::Hello`,
/// is bound to a session id, then the steady-state pump takes over.
fn accept_new(
    listener: &TcpListener,
    clients: &mut Vec<ClientConn>,
    sessions: &mut SessionTable,
    sidecar_slot: Option<&crate::SidecarSlot>,
    single_process_mode: bool,
    pinned_session_id: Option<&str>,
    tracker: Option<&crate::SessionTracker>,
) {
    loop {
        match listener.accept() {
            Ok((stream, peer)) => {
                eprintln!(
                    "[dev-server] client connected: {} ({} active)",
                    peer,
                    clients.len() + 1
                );
                let _ = stream.set_nonblocking(false);
                let mut ws = match tungstenite::accept(stream) {
                    Ok(ws) => ws,
                    Err(e) => {
                        eprintln!("[dev-server] handshake failed: {}", e);
                        continue;
                    }
                };

                // Read the client's Hello synchronously. Pre-v5 clients
                // that don't populate `identity` still parse cleanly
                // (serde default).
                let _ = ws
                    .get_ref()
                    .set_read_timeout(Some(HANDSHAKE_DEADLINE));
                let hello = match read_hello(&mut ws) {
                    Ok(h) => h,
                    Err(e) => {
                        eprintln!(
                            "[dev-server] {} no Hello within {:?}: {} — dropping",
                            peer, HANDSHAKE_DEADLINE, e
                        );
                        let _ = ws.close(None);
                        continue;
                    }
                };
                let (app_name, color_scheme, _initial_url, identity, viewport) = hello;
                let label = identity
                    .device_label
                    .as_deref()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("{:?}", identity.platform));
                eprintln!(
                    "[dev-server] app hello: {} (platform={:?} label={:?})",
                    app_name, identity.platform, label,
                );

                // Decide the session id. Server-only decision: either a
                // pinned id (single-process or Shared mode) or a fresh
                // server-minted id (PerClient mode).
                let session_id = match pinned_session_id {
                    Some(id) => id.to_string(),
                    None => sessions.mint_anonymous(&identity),
                };

                // Bring the session into existence on the host side.
                // In sidecar mode we also tell the sidecar to spin up
                // the matching author runtime thread.
                let was_created;
                {
                    let (_state, created) = sessions.get_or_create(&session_id);
                    was_created = created;
                }
                if was_created {
                    eprintln!(
                        "[dev-server] session {:?} created (peer={})",
                        session_id, peer
                    );
                    if let Some(t) = tracker {
                        t.insert(&session_id);
                        // Stash the client's reported viewport so
                        // `replay_sessions_to_sidecar` can re-emit it
                        // when a sidecar respawn (hot-patch fallback)
                        // forces session re-creation.
                        t.set_viewport(&session_id, viewport);
                    }
                    if !single_process_mode {
                        if let Some(slot) = sidecar_slot {
                            if let Ok(guard) = slot.lock() {
                                if let Some(sidecar) = guard.as_ref() {
                                    sidecar.send(crate::SidecarIn::CreateSession {
                                        session: session_id.clone(),
                                        viewport,
                                    });
                                }
                            }
                        }
                    }
                } else {
                    eprintln!(
                        "[dev-server] session {:?} attaching peer {}",
                        session_id, peer
                    );
                }

                // Reflect the client's color scheme onto its session's
                // mirror so author code reading `color_scheme()` on the
                // sidecar's recorder side sees the right value. (In
                // sidecar mode this affects the mirror only — the
                // sidecar's per-session recorder is updated through the
                // forwarded ColorSchemeChanged event.)
                if let Some(state) = sessions.get(&session_id) {
                    state.mirror.set_color_scheme(wire_color_scheme_to_core(color_scheme));
                }

                if send_hello(&mut ws, &session_id).is_err() {
                    continue;
                }

                // Snapshot the live scene at the moment we accept, and
                // capture the cursor at the same instant.
                let (cursor_at_snapshot, snapshot, epoch_at_snapshot) = {
                    let Some(state) = sessions.get(&session_id) else {
                        eprintln!(
                            "[dev-server] session {:?} disappeared between create and snapshot",
                            session_id
                        );
                        continue;
                    };
                    (
                        state.mirror.command_count(),
                        state.mirror.snapshot(),
                        state.mirror.epoch(),
                    )
                };
                if !snapshot.is_empty() {
                    eprintln!(
                        "[dev-server] catching up {} (session={}) with {} commands",
                        peer,
                        session_id,
                        snapshot.len(),
                    );
                    if send(&mut ws, &DevToApp::Commands(snapshot)).is_err() {
                        continue;
                    }
                }
                let _ = ws.get_mut().set_nonblocking(true);
                // Clear the read timeout we set during handshake. The
                // steady-state loop polls non-blocking; a lingering
                // timeout would silently force WouldBlock returns that
                // we treat as transient — wasted work.
                let _ = ws.get_ref().set_read_timeout(None);
                clients.push(ClientConn {
                    ws,
                    cursor: cursor_at_snapshot,
                    epoch: epoch_at_snapshot,
                    peer: peer.to_string(),
                    session: session_id,
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

/// Wait synchronously for the first `AppToDev::Hello` frame on a
/// newly-accepted socket. Returns the parsed Hello fields as a tuple
/// keyed positionally (app_name, color_scheme, initial_url, identity,
/// viewport).
fn read_hello(
    ws: &mut WebSocket<TcpStream>,
) -> Result<
    (
        String,
        WireColorScheme,
        Option<String>,
        ClientIdentity,
        Option<wire::WireViewport>,
    ),
    TransportError,
> {
    loop {
        match ws.read() {
            Ok(Message::Text(t)) => {
                let msg: AppToDev = serde_json::from_str(t.as_str())
                    .map_err(|e| TransportError::Decode(e.to_string()))?;
                return extract_hello(msg);
            }
            Ok(Message::Binary(b)) => {
                let msg: AppToDev = serde_json::from_slice(&b)
                    .map_err(|e| TransportError::Decode(e.to_string()))?;
                return extract_hello(msg);
            }
            Ok(Message::Ping(p)) => {
                let _ = ws.send(Message::Pong(p));
            }
            Ok(Message::Close(_)) => {
                return Err(TransportError::Decode("peer closed before Hello".into()));
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if e.kind() == ErrorKind::WouldBlock
                    || e.kind() == ErrorKind::TimedOut =>
            {
                return Err(TransportError::Decode(
                    "no Hello within handshake deadline".into(),
                ));
            }
            Err(e) => return Err(TransportError::Tungstenite(e)),
        }
    }
}

fn extract_hello(
    msg: AppToDev,
) -> Result<
    (
        String,
        WireColorScheme,
        Option<String>,
        ClientIdentity,
        Option<wire::WireViewport>,
    ),
    TransportError,
> {
    match msg {
        AppToDev::Hello {
            app_name,
            color_scheme,
            initial_url,
            identity,
            viewport,
        } => Ok((app_name, color_scheme, initial_url, identity, viewport)),
        other => Err(TransportError::Decode(format!(
            "expected AppToDev::Hello, got {:?}",
            other
        ))),
    }
}

/// Drain any pending WebSocket frames from each connected client.
/// Disconnected / errored clients are removed.
fn poll_reads(
    clients: &mut Vec<ClientConn>,
    sidecar_slot: Option<&crate::SidecarSlot>,
    single_process_mode: bool,
    sessions: &SessionTable,
    tracker: Option<&crate::SessionTracker>,
) {
    let mut keep = Vec::with_capacity(clients.len());
    for mut client in clients.drain(..) {
        let mut alive = true;
        loop {
            match client.ws.read() {
                Ok(Message::Text(t)) => match serde_json::from_str::<AppToDev>(t.as_str()) {
                    Ok(msg) => handle_app_msg(
                        &client.session,
                        msg,
                        sidecar_slot,
                        single_process_mode,
                        sessions,
                        tracker,
                    ),
                    Err(e) => eprintln!("[dev-server] decode error: {}", e),
                },
                Ok(Message::Binary(b)) => match serde_json::from_slice::<AppToDev>(&b) {
                    Ok(msg) => handle_app_msg(
                        &client.session,
                        msg,
                        sidecar_slot,
                        single_process_mode,
                        sessions,
                        tracker,
                    ),
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

fn handle_app_msg(
    client_session: &str,
    msg: AppToDev,
    sidecar_slot: Option<&crate::SidecarSlot>,
    single_process_mode: bool,
    sessions: &SessionTable,
    tracker: Option<&crate::SessionTracker>,
) {
    // Hello can arrive again only if a client mis-uses the protocol;
    // log + drop.
    if matches!(msg, AppToDev::Hello { .. }) {
        eprintln!("[dev-server] late Hello on session {}; ignoring", client_session);
        return;
    }

    // Snapshot viewport from ViewportChanged so the SessionTracker
    // can replay it after a sidecar respawn. Forwarding the event
    // to the sidecar still happens below — this is a peek, not a
    // consume.
    if let AppToDev::ViewportChanged { width, height } = &msg {
        if let Some(t) = tracker {
            t.set_viewport(client_session, Some(wire::WireViewport {
                width: *width,
                height: *height,
            }));
        }
    }

    if !single_process_mode {
        if let Some(slot) = sidecar_slot {
            if let Ok(guard) = slot.lock() {
                if let Some(sidecar) = guard.as_ref() {
                    sidecar.send(crate::SidecarIn::Event {
                        session: client_session.to_string(),
                        event: msg,
                    });
                    return;
                }
            }
            // Sidecar slot exists but is empty (rebuild in flight).
            return;
        }
    }

    // No sidecar — single-process mode. Dispatch against the primary
    // mirror, which IS the host's live recorder.
    let Some(state) = sessions.get(client_session) else {
        eprintln!(
            "[dev-server] handle_app_msg: no session {:?}; dropping",
            client_session
        );
        return;
    };
    let recorder = &state.mirror;
    match msg {
        AppToDev::Hello { .. } => {} // handled above
        AppToDev::Event { handler, args } => {
            let _ = recorder.dispatch_event(handler, args);
        }
        AppToDev::StateChanged { node, bit, on } => {
            let _ = recorder.dispatch_state(node, bit, on);
        }
        AppToDev::ColorSchemeChanged { scheme } => {
            recorder.set_color_scheme(wire_color_scheme_to_core(scheme));
        }
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
        AppToDev::RequestFrame { .. } => {
            // Single-process mode has no sidecar to forward to and
            // no per-thread animation clock to drive — local-render
            // mode doesn't use the client-driven raf path. Drop
            // silently. (The sidecar branch above this fn forwards
            // RequestFrame through to the session thread.)
        }
        AppToDev::ViewportChanged { .. } => {
            // Single-process mode: the host process IS the renderer,
            // so the browser's resize events don't need to flow to
            // the sidecar (there isn't one). Drop silently. The
            // sidecar branch above forwards ViewportChanged through
            // to the session thread.
        }
        AppToDev::Error { message } => {
            eprintln!("[dev-server] app reported error: {}", message);
        }
    }
}

/// Ship every command appended to each session's mirror since each
/// client's cursor. Disconnected clients are pruned.
fn broadcast_new_commands(clients: &mut Vec<ClientConn>, sessions: &SessionTable) {
    let mut keep = Vec::with_capacity(clients.len());
    for mut client in clients.drain(..) {
        let Some(state) = sessions.get(&client.session) else {
            // Session vanished (probably SessionEnded). Drop the
            // client; SessionEnded already issued a close.
            eprintln!(
                "[dev-server] {} session {:?} gone; dropping client",
                client.peer, client.session
            );
            continue;
        };
        let mirror = &state.mirror;
        let new_count = mirror.command_count();
        let mirror_epoch = mirror.epoch();

        if client.epoch != mirror_epoch {
            // Scene reset (hot-patch). Send fresh snapshot.
            let snapshot = mirror.snapshot();
            eprintln!(
                "[dev-server] {} (session={}) epoch advanced ({} → {}); resnap {} cmds",
                client.peer,
                client.session,
                client.epoch,
                mirror_epoch,
                snapshot.len(),
            );
            match send(&mut client.ws, &DevToApp::Commands(snapshot)) {
                Ok(()) | Err(TransportError::WouldBlockBuffered) => {
                    // Either fully written, or buffered inside
                    // tungstenite — a future `flush()` (next tick)
                    // pushes the rest. Either way the message is
                    // committed; advance bookkeeping.
                    client.cursor = new_count;
                    client.epoch = mirror_epoch;
                }
                Err(e) => {
                    eprintln!("[dev-server] {} send failed: {} ; dropping", client.peer, e);
                    continue;
                }
            }
        } else if new_count > client.cursor {
            let cmds = mirror.commands_since(client.cursor);
            match send(&mut client.ws, &DevToApp::Commands(cmds)) {
                Ok(()) | Err(TransportError::WouldBlockBuffered) => {
                    client.cursor = new_count;
                }
                Err(e) => {
                    eprintln!("[dev-server] {} send failed: {} ; dropping", client.peer, e);
                    continue;
                }
            }
        } else {
            // No new data this tick. Try to push out anything
            // tungstenite has buffered from a prior WouldBlock —
            // otherwise it'd sit there indefinitely on a quiet
            // socket. WouldBlock from flush is benign.
            let _ = flush_best_effort(&mut client.ws);
        }
        keep.push(client);
    }
    *clients = keep;
}

fn send_hello<S>(ws: &mut WebSocket<S>, session: &str) -> Result<(), TransportError>
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
        session: session.to_string(),
    };
    send(ws, &hello)
}

fn send<S>(ws: &mut WebSocket<S>, msg: &DevToApp) -> Result<(), TransportError>
where
    S: std::io::Read + std::io::Write,
{
    let bytes = serde_json::to_vec(msg).map_err(|e| TransportError::Encode(e.to_string()))?;
    match ws.send(Message::Binary(bytes.into())) {
        Ok(()) => Ok(()),
        // `send()` internally calls `write()` (buffers the frame
        // into tungstenite's outgoing queue) followed by `flush()`
        // (drains the queue onto the underlying socket). On a
        // non-blocking socket, flush returns WouldBlock when the
        // kernel send buffer can't take the whole frame in one go —
        // e.g. a big initial render of a complex scene. The frame is
        // safely buffered inside tungstenite; the next tick's
        // [`flush_best_effort`] drains it. Treat this as a deferred
        // success rather than a fatal send error.
        Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
            Err(TransportError::WouldBlockBuffered)
        }
        Err(e) => Err(TransportError::Tungstenite(e)),
    }
}

/// Drain any bytes tungstenite has buffered internally from a prior
/// WouldBlock send. WouldBlock from `flush` itself is benign — the
/// kernel still can't take more right now; we'll try again next tick.
/// Any non-WouldBlock error is propagated so the caller can decide
/// whether to drop the client.
fn flush_best_effort<S>(ws: &mut WebSocket<S>) -> Result<(), TransportError>
where
    S: std::io::Read + std::io::Write,
{
    match ws.flush() {
        Ok(()) => Ok(()),
        Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(TransportError::Tungstenite(e)),
    }
}

/// Like [`serve`] but also drives a Robot bridge handle once per
/// tick. See module-level docs for caveats around Robot + multi-session.
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
        let _ = self.daemon.shutdown();
    }
}

fn advertise_mdns(app_id: &str, port: u16) -> Result<MdnsHandle, Box<dyn std::error::Error>> {
    let daemon = ServiceDaemon::new()?;
    let pid = std::process::id();
    let app_id_label = app_id.replace('.', "-");
    let instance_name = format!("{}-{}", app_id_label, pid);
    let hostname = format!("idealyst-{}-{}.local.", app_id_label, pid);
    let proto = PROTOCOL_VERSION.to_string();
    // TXT carries an `aas_sessions=multi` tag so older clients can detect
    // session-aware servers — useful when we eventually add per-session
    // browse UI; harmless for unaware clients (they just ignore it).
    let txt: [(&str, &str); 3] = [
        ("app_id", app_id),
        ("proto", proto.as_str()),
        ("aas_sessions", "multi"),
    ];
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

#[derive(Debug)]
pub enum TransportError {
    Io(std::io::Error),
    Handshake(tungstenite::handshake::HandshakeError<tungstenite::ServerHandshake<std::net::TcpStream, tungstenite::handshake::server::NoCallback>>),
    Tungstenite(tungstenite::Error),
    Encode(String),
    Decode(String),
    /// `send()` accepted the message into tungstenite's outgoing
    /// queue but the underlying non-blocking socket couldn't take it
    /// all this tick. Callers should treat this as a deferred
    /// success: advance bookkeeping (the message *is* committed) and
    /// let the next tick's `flush_best_effort` finish the I/O.
    WouldBlockBuffered,
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
            TransportError::WouldBlockBuffered => {
                write!(f, "send buffered (would block, will flush next tick)")
            }
        }
    }
}

impl std::error::Error for TransportError {}
