//! Cross-platform AAS-client run-loop.
//!
//! The original AAS-iOS implementation lived entirely in
//! `examples/hello-ios-aas/src/lib.rs` and bundled together a few
//! genuinely platform-specific concerns (the `ios_main` C entry,
//! `dispatch_async_f` main-thread scheduling, iOS-specific layout
//! pass) with a much larger pile of platform-agnostic logic
//! (mDNS discovery, WebSocket connect/reconnect, inbound channel
//! drain, message dispatch). This module is the platform-agnostic
//! pile, lifted into the shared `dev-client` crate so iOS, Android,
//! and desktop hosts can all consume it.
//!
//! The split between this and the platform shell:
//!
//! - **Here:** spawn the WebSocket worker thread, browse Bonjour
//!   for a matching `app_id`, open the connection, ferry frames
//!   onto an `mpsc::Receiver<DevToApp>`, and provide a `drain()`
//!   method that pulls them off and applies them through an
//!   [`AasClient`]`<B>`.
//! - **Platform shell:** create the backend, set up its host view,
//!   schedule a periodic `drain()` call on whatever the platform's
//!   "main thread" or render thread is, and run any post-batch
//!   work the backend requires (e.g. an iOS layout pass).
//!
//! The shell owns the `AasShell` and holds it across periodic
//! ticks. Each tick consumes whatever the worker has shipped over.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use framework_core::Backend;
use wire::{AppToDev, DevToApp};

use crate::{discover_blocking, AasClient, OutboundSender};

/// Bundle of state the platform shell holds across drain ticks. Wrap
/// in `Rc<...>` (or a thread-local) so the periodic callback can
/// reach it without taking the AAS client through the FFI boundary
/// every time.
///
/// `client` is `Rc<RefCell<...>>` because the host can hand out
/// references for re-entrant access (e.g. `backend_mut()` after a
/// batch apply, to run a layout pass).
pub struct AasShell<B: Backend + 'static> {
    pub client: Rc<RefCell<AasClient<B>>>,
    inbound: mpsc::Receiver<DevToApp>,
}

impl<B: Backend + 'static> AasShell<B> {
    /// Build the shell, install an outbound channel that survives
    /// reconnects, and spawn the background WebSocket worker that
    /// discovers the dev-server via Bonjour.
    ///
    /// The returned shell isn't `Send` (the `Rc<RefCell<AasClient>>`
    /// is `!Send`), reflecting the architectural assumption that
    /// `drain()` is always invoked on the platform's render /
    /// main thread. The worker thread runs the blocking transport
    /// and communicates only over channels.
    pub fn spawn(backend: B, app_id: String) -> Self {
        Self::spawn_inner(backend, Target::Discover(app_id))
    }

    /// Same as [`spawn`] but skips Bonjour discovery and connects
    /// directly to a fixed WebSocket URL. Used for platforms whose
    /// network stack can't see the host's mDNS broadcasts — most
    /// commonly the Android Studio emulator, where the CLI sets up
    /// an `adb reverse` tunnel and passes the post-tunnel URL
    /// (`ws://127.0.0.1:<port>`) here.
    ///
    /// On disconnect the worker reconnects to the same URL — there's
    /// no rediscovery loop because we never browsed in the first
    /// place. If the dev-server restarted on a new port, that's a
    /// configuration mismatch the caller has to detect and re-spawn
    /// the shell with the updated URL.
    pub fn spawn_with_url(backend: B, url: String) -> Self {
        Self::spawn_inner(backend, Target::Url(url))
    }

    fn spawn_inner(backend: B, target: Target) -> Self {
        let outbound = OutboundSender::new();
        let (inbound_tx, inbound_rx) = mpsc::channel::<DevToApp>();
        let (outbound_tx, outbound_rx) = mpsc::channel::<AppToDev>();
        outbound.set(outbound_tx);

        let client = Rc::new(RefCell::new(AasClient::new(backend, outbound.clone())));

        std::thread::spawn(move || {
            ws_worker_loop(target, inbound_tx, outbound_rx);
        });

        Self { client, inbound: inbound_rx }
    }

    /// Drain any pending dev→app messages and apply them through
    /// the [`AasClient`]. Returns `true` if at least one message
    /// was processed — callers use that to gate per-batch follow-up
    /// work like an iOS layout pass.
    ///
    /// Safe to call from a periodic timer; cheap when the channel
    /// is empty.
    pub fn drain(&self) -> bool {
        let mut msgs: Vec<DevToApp> = Vec::new();
        while let Ok(msg) = self.inbound.try_recv() {
            msgs.push(msg);
        }
        if msgs.is_empty() {
            return false;
        }
        let mut client = self.client.borrow_mut();
        for msg in msgs {
            apply_dev_msg(&mut client, msg);
        }
        true
    }
}

/// Decide what to do with one inbound `DevToApp` message. Split out
/// of [`AasShell::drain`] so per-message handling stays trivial.
fn apply_dev_msg<B: Backend>(client: &mut AasClient<B>, msg: DevToApp) {
    match msg {
        DevToApp::Hello { .. } => {}
        DevToApp::Commands(cmds) => {
            if let Err(e) = client.apply_batch(cmds) {
                eprintln!("[aas-shell] replay error: {:?}", e);
            }
        }
        DevToApp::Rebuilding => eprintln!("[aas-shell] dev rebuilding…"),
        DevToApp::Error { message } => eprintln!("[aas-shell] dev error: {}", message),
        DevToApp::ThemeChanged { .. } => {}
    }
}

/// How the worker thread should locate the dev-server. `Discover`
/// is the LAN-friendly default; `Url` is the override the CLI uses
/// when the platform's network can't see Bonjour (Android emulator).
enum Target {
    Discover(String),
    Url(String),
}

/// Background-thread worker. Either browses Bonjour for the matching
/// `app_id` or connects directly to a fixed URL, opens the
/// WebSocket, sends a Hello, then pumps frames between socket and
/// channels. On disconnect, the discover path loops back to
/// `discover_blocking` so a dev-server that restarted on a fresh
/// port is picked up transparently; the url path reconnects to the
/// same address.
fn ws_worker_loop(
    target: Target,
    inbound_tx: mpsc::Sender<DevToApp>,
    outbound_rx: mpsc::Receiver<AppToDev>,
) {
    match &target {
        Target::Discover(app_id) => {
            eprintln!("[aas-shell] worker starting; browsing for app_id={:?}", app_id);
        }
        Target::Url(url) => {
            eprintln!("[aas-shell] worker starting; direct url={:?}", url);
        }
    }
    loop {
        let url = match &target {
            Target::Discover(app_id) => discover_blocking(app_id, Duration::from_secs(3)),
            Target::Url(u) => u.clone(),
        };
        eprintln!("[aas-shell] connecting to {}", url);
        let (mut ws, _) = match tungstenite::connect(&url) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[aas-shell] connect failed: {} — retrying in 1s", e);
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        if let tungstenite::stream::MaybeTlsStream::Plain(s) = ws.get_ref() {
            // Short read timeout so we can also poll the outbound
            // channel in the same loop without spawning a second
            // thread. 50ms keeps the loop responsive without
            // burning CPU.
            let _ = s.set_read_timeout(Some(Duration::from_millis(50)));
        }

        let hello = AppToDev::Hello {
            app_name: "aas-client".into(),
            color_scheme: wire::WireColorScheme::Auto,
            initial_url: None,
        };
        let _ = ws_send(&mut ws, &hello);
        eprintln!("[aas-shell] connected");

        let _ = run_ws_session(&mut ws, &inbound_tx, &outbound_rx);
        eprintln!("[aas-shell] disconnected; rediscovering");
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn ws_send<S>(
    ws: &mut tungstenite::WebSocket<S>,
    msg: &AppToDev,
) -> Result<(), tungstenite::Error>
where
    S: std::io::Read + std::io::Write,
{
    let bytes = serde_json::to_vec(msg).expect("encode AppToDev");
    ws.send(tungstenite::Message::Binary(bytes.into()))
}

fn run_ws_session<S>(
    ws: &mut tungstenite::WebSocket<S>,
    inbound_tx: &mpsc::Sender<DevToApp>,
    outbound_rx: &mpsc::Receiver<AppToDev>,
) -> Result<(), tungstenite::Error>
where
    S: std::io::Read + std::io::Write,
{
    use std::io::ErrorKind;
    use tungstenite::Message;
    loop {
        match ws.read() {
            Ok(Message::Binary(b)) => match serde_json::from_slice::<DevToApp>(&b) {
                Ok(msg) => {
                    if inbound_tx.send(msg).is_err() {
                        return Ok(());
                    }
                }
                Err(e) => eprintln!("[aas-shell] decode error: {}", e),
            },
            Ok(Message::Text(t)) => match serde_json::from_str::<DevToApp>(t.as_str()) {
                Ok(msg) => {
                    if inbound_tx.send(msg).is_err() {
                        return Ok(());
                    }
                }
                Err(e) => eprintln!("[aas-shell] decode error: {}", e),
            },
            Ok(Message::Close(_)) => return Ok(()),
            Ok(Message::Ping(p)) => {
                let _ = ws.send(Message::Pong(p));
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
            Err(
                tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed,
            ) => return Ok(()),
            Err(e) => return Err(e),
        }
        while let Ok(msg) = outbound_rx.try_recv() {
            ws_send(ws, &msg)?;
        }
    }
}
