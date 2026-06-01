//! Cross-platform runtime-server-client run-loop.
//!
//! The original runtime-server-iOS implementation lived entirely in
//! `examples/hello-ios-runtime-server/src/lib.rs` and bundled together a few
//! genuinely platform-specific concerns (the `ios_main` C entry,
//! `dispatch_async_f` main-thread scheduling, iOS-specific layout
//! pass) with a much larger pile of platform-agnostic logic
//! (WebSocket connect/reconnect, inbound channel drain, message
//! dispatch). This module is the platform-agnostic pile, lifted into
//! the shared `runtime-server-shell-native` crate so iOS, Android, and
//! desktop hosts can all consume it.
//!
//! The split between this and the platform shell:
//!
//! - **Here:** spawn the WebSocket worker thread against a CLI-baked
//!   URL, open the connection, ferry frames onto an
//!   `mpsc::Receiver<DevToApp>`, and provide a `drain()` method that
//!   pulls them off and applies them through an [`RuntimeServerClient`]`<B>`.
//! - **Platform shell:** create the backend, set up its host view,
//!   schedule a periodic `drain()` call on whatever the platform's
//!   "main thread" or render thread is, and run any post-batch
//!   work the backend requires (e.g. an iOS layout pass).
//!
//! The shell owns the `RuntimeServerShell` and holds it across periodic
//! ticks. Each tick consumes whatever the worker has shipped over.
//!
//! ## Endpoint resolution
//!
//! The shell does NOT discover the dev-server itself — discovery was
//! historically mDNS / Bonjour, which was unreliable across networks
//! and required platform-specific permissions (iOS `NSBonjourServices`,
//! Android `MulticastLock`). Instead the CLI bakes the endpoint URL
//! into the wrapper build via `IDEALYST_DEV_ENDPOINT=ws://host:port`
//! and the wrapper passes the resolved URL straight into
//! [`RuntimeServerShell::spawn`]. See [`resolve_endpoint`].

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use runtime_core::Backend;
use wire::{AppToDev, DevToApp};

use dev_client::{RuntimeServerClient, OutboundSender};

/// Env var the wrapper-template build-script / CLI sets to point the
/// runtime-server client at a dev-server URL. Format: `ws://host:port`.
/// Read at *wrapper-build* time on iOS / Android (baked into the app
/// binary), at *runtime* on macOS / terminal / wgpu (set by the CLI
/// on the spawned child). See [`resolve_endpoint`].
pub const ENDPOINT_ENV: &str = "IDEALYST_DEV_ENDPOINT";

/// Read [`ENDPOINT_ENV`] at runtime. Returns `None` when unset — the
/// platform wrapper should handle that case with an explicit panic /
/// log so the failure is loud rather than "app silently never
/// connects." Most wrappers bake the value at build time and reach for
/// [`endpoint_or_panic`] instead.
pub fn resolve_endpoint() -> Option<String> {
    std::env::var(ENDPOINT_ENV).ok().filter(|s| !s.is_empty())
}

/// Like [`resolve_endpoint`] but panics with a clear message when the
/// env var is missing. Use from wrapper-template main / FFI entry
/// points where there's no sensible recovery — the only fix is to
/// rebuild via `idealyst dev`, which sets the env var.
pub fn endpoint_or_panic() -> String {
    resolve_endpoint().unwrap_or_else(|| {
        panic!(
            "{ENDPOINT_ENV} is not set — this build was launched outside of \
             `idealyst dev`. Rebuild via `idealyst dev` (which bakes the \
             dev-server URL into the wrapper) to use runtime-server mode."
        )
    })
}

/// Bundle of state the platform shell holds across drain ticks. Wrap
/// in `Rc<...>` (or a thread-local) so the periodic callback can
/// reach it without taking the runtime-server client through the FFI boundary
/// every time.
///
/// `client` is `Rc<RefCell<...>>` because the host can hand out
/// references for re-entrant access (e.g. `backend_mut()` after a
/// batch apply, to run a layout pass).
pub struct RuntimeServerShell<B: Backend + 'static> {
    pub client: Rc<RefCell<RuntimeServerClient<B>>>,
    inbound: mpsc::Receiver<DevToApp>,
    /// Last viewport reported via [`Self::report_viewport`] (or the
    /// initial Hello value). Used to skip redundant `ViewportChanged`
    /// sends when the platform shell calls `report_viewport` on every
    /// drain tick — common when the shell doesn't have a layout-change
    /// listener and instead just samples on each frame.
    last_reported_viewport: RefCell<Option<wire::WireViewport>>,
}

/// Optional knobs for [`RuntimeServerShell::spawn_with_options`].
/// Session assignment is entirely server-side — these options are
/// about how the client describes *itself* to the server (used by
/// logs and the future session-picker dev tool).
#[derive(Default, Clone, Debug)]
pub struct RuntimeServerShellOptions {
    /// Platform this client runs on. Sent in `AppToDev::Hello.identity`
    /// so the server's logs and the future session-picker dev tool can
    /// distinguish "iPhone" from "Pixel". The native shell defaults to
    /// [`wire::WirePlatform::Other`] — concrete iOS / Android wrappers
    /// override this with the real platform constant.
    pub platform: wire::WirePlatform,
    /// Free-form device label for display ("iPhone 15 Pro Simulator",
    /// "Pixel 8", "MacBook Air (M2)"). Server falls back to the
    /// platform name when this is `None`.
    pub device_label: Option<String>,
    /// Initial viewport in CSS pixels the client is rendering into.
    /// Shipped in `AppToDev::Hello.viewport`; the sidecar plugs it
    /// into `RecordingViewOps::frame()` so author code reading
    /// `page_ref.with(|h| h.frame())` sees the *real* native window
    /// size instead of the hardcoded `393×800` fallback. Pre-fix
    /// every native runtime-server shell shipped `None`, so welcome's planet-
    /// orbit math (and any other code that reads the viewport)
    /// rendered for a phantom 393×800 mobile canvas — visible as
    /// off-aligned content on any client whose window isn't that
    /// exact size. iOS / Android wrappers should fill this from
    /// `UIView.bounds` / the root `View`'s size after layout; the
    /// macOS wrapper supplies the `NSWindow.contentView.bounds`.
    pub viewport: Option<wire::WireViewport>,
}

/// How [`RuntimeServerShell::spawn_inner`] receives the backend.
/// `ByValue` is what iOS / Android / macOS use (their backend
/// lives exclusively inside the shell). `Shared` is what the
/// wgpu sim path uses to keep the same `Rc<RefCell<WgpuBackend>>`
/// in the shell AND in `render-wgpu::Host` simultaneously.
enum SpawnBackend<B: Backend> {
    ByValue(B),
    Shared(Rc<RefCell<B>>),
}

impl<B: Backend + 'static> RuntimeServerShell<B> {
    /// Build the shell, install an outbound channel that survives
    /// reconnects, and spawn the background WebSocket worker that
    /// connects to `url` (`ws://host:port`).
    ///
    /// The returned shell isn't `Send` (the `Rc<RefCell<RuntimeServerClient>>`
    /// is `!Send`), reflecting the architectural assumption that
    /// `drain()` is always invoked on the platform's render /
    /// main thread. The worker thread runs the blocking transport
    /// and communicates only over channels.
    pub fn spawn(backend: B, url: String) -> Self {
        Self::spawn_inner(SpawnBackend::ByValue(backend), url, RuntimeServerShellOptions::default())
    }

    /// Same as [`spawn`] but lets the caller pass [`RuntimeServerShellOptions`]
    /// (platform tag, device label, initial viewport).
    pub fn spawn_with_options(
        backend: B,
        url: String,
        options: RuntimeServerShellOptions,
    ) -> Self {
        Self::spawn_inner(SpawnBackend::ByValue(backend), url, options)
    }

    /// Spawn around a pre-shared backend handle. Used by the wgpu
    /// sim runtime-server path, where the same `Rc<RefCell<WgpuBackend>>`
    /// is also held by `render-wgpu::Host` so its renderer can read
    /// the scene on every redraw. iOS / Android / macOS don't share
    /// (their backends live exclusively inside the shell) so they
    /// keep using the by-value [`Self::spawn_with_options`] form
    /// which wraps internally.
    pub fn spawn_with_shared_backend(
        backend: Rc<RefCell<B>>,
        url: String,
        options: RuntimeServerShellOptions,
    ) -> Self {
        Self::spawn_inner(
            SpawnBackend::Shared(backend),
            url,
            options,
        )
    }

    fn spawn_inner(backend: SpawnBackend<B>, url: String, options: RuntimeServerShellOptions) -> Self {
        let outbound = OutboundSender::new();
        let (inbound_tx, inbound_rx) = mpsc::channel::<DevToApp>();
        let (outbound_tx, outbound_rx) = mpsc::channel::<AppToDev>();
        outbound.set(outbound_tx);

        let client = Rc::new(RefCell::new(match backend {
            SpawnBackend::ByValue(b) => RuntimeServerClient::new(b, outbound.clone()),
            SpawnBackend::Shared(rc) => RuntimeServerClient::new_with_shared(rc, outbound.clone()),
        }));

        // Seed the viewport-change tracker with whatever shipped in
        // the Hello so the first `report_viewport` call only emits
        // a `ViewportChanged` if the platform shell discovered a
        // different size post-layout. Without seeding, every
        // first-call would emit a redundant message.
        let initial_viewport = options.viewport.clone();
        let options_for_worker = options;
        std::thread::spawn(move || {
            ws_worker_loop(url, inbound_tx, outbound_rx, options_for_worker);
        });

        Self {
            client,
            inbound: inbound_rx,
            last_reported_viewport: RefCell::new(initial_viewport),
        }
    }

    /// Drain any pending dev→app messages and apply them through
    /// the [`RuntimeServerClient`], then push an `AppToDev::RequestFrame` so
    /// the sidecar advances its animation clock + scheduler one
    /// tick. Returns `true` if at least one inbound message was
    /// processed — callers use that to gate per-batch follow-up
    /// work like an iOS layout pass.
    ///
    /// **Why the `RequestFrame`:** animation cadence is
    /// **client-driven** on the sidecar side. `tick_animations` only
    /// runs in response to an inbound `RequestFrame`; without one,
    /// timeline tweens (welcome's intro fade-in, page-bg dark wash)
    /// never start and the AV.bind subscriptions never emit
    /// `SetAnimated*` commands. Pre-fix every native runtime-server shell
    /// (iOS / Android / macOS) had its session sitting at "ready"
    /// while the user saw a blank initial frame — only the web
    /// shell sent `RequestFrame` (from its raf pump in
    /// `backend-web::dev_transport`). Pushing one here on every
    /// drain tick pairs the inbound + outbound rates 1:1, mirrors
    /// the web behavior, and costs ~16 bytes per tick over the WS.
    ///
    /// `dt_ms` defaults to 16 — the typical 60fps cadence the
    /// drain timer is scheduled at. If a caller drives the drain
    /// at a different rate it can use [`Self::drain_with_dt_ms`].
    ///
    /// Safe to call from a periodic timer; cheap when the channel
    /// is empty.
    pub fn drain(&self) -> bool {
        self.drain_with_dt_ms(16)
    }

    /// As [`Self::drain`] but lets the caller specify the
    /// `dt_ms` value used in the trailing `RequestFrame`. Useful
    /// for callers whose drain cadence isn't ~16ms.
    /// One-stop main-thread tick: report viewport, drain inbound
    /// (which sends `RequestFrame`), drive a layout pass on the
    /// backend if anything was applied. Single call covers every
    /// per-frame responsibility a native runtime-server shell has.
    ///
    /// Each of the three operations is independently idempotent
    /// / cheap — calling `tick` on an empty inbound queue with an
    /// unchanged viewport just sends one `RequestFrame` (the
    /// animation pacer the sidecar needs). Calling it on a layout
    /// pass with no new commands skips the `run_layout` invocation.
    ///
    /// Pre-fix every platform shell wrote its own version of this:
    /// iOS called `shell.drain()` then `backend_mut().run_layout()`
    /// in its dispatch trampoline; Android called `shell.drain()`
    /// then conditionally `backend_mut().run_layout()`; macOS called
    /// `shell.drain()` and relied on `finish()` to layout-as-it-
    /// applies. None of them reported viewport per-tick (only the
    /// initial Hello) — Android grew that path latest after we
    /// noticed welcome's planet orbit using the 393×800 fallback.
    /// All three converge on this single entry point now.
    pub fn tick(&self, viewport: Option<wire::WireViewport>) -> bool {
        if let Some(vp) = viewport {
            self.report_viewport(vp);
        }
        let had_inbound = self.drain();
        if had_inbound {
            // `Backend::run_layout` is a no-op by default; backends
            // whose `finish()` synchronously applies frames (macOS,
            // web) leave it at the default. iOS + Android override
            // to drive the deferred path their normal scheduler
            // can't reach in runtime-server mode.
            // Two borrows: the client first (read-only — we only
            // need it to get the backend Rc), then the backend
            // (mutable for the layout pass). Doing it as one chain
            // would hold the client borrow across the layout,
            // blocking the next `apply_batch` if `run_layout`
            // re-enters via a backend method that needs the
            // wire client.
            let backend_rc = self.client.borrow().backend().clone();
            backend_rc.borrow_mut().run_layout();
        }
        had_inbound
    }

    /// Report the host view's current viewport to the sidecar. If
    /// `viewport` differs from whatever was last reported (including
    /// the initial Hello), emits `AppToDev::ViewportChanged` so the
    /// sidecar's `RecordingViewOps::frame()` reflects the live size.
    /// Cheap when nothing changed (no message sent).
    ///
    /// Platform shells whose host view doesn't have valid bounds at
    /// `attach()` time (the Android case — `View.getWidth()` returns
    /// 0 until the first layout pass) should call this on each drain
    /// tick with the freshly-sampled `view.bounds`. The sidecar's
    /// 393×800 fallback only persists until this call lands.
    pub fn report_viewport(&self, viewport: wire::WireViewport) {
        let mut last = self.last_reported_viewport.borrow_mut();
        if last.as_ref() == Some(&viewport) {
            return;
        }
        let _ = self
            .client
            .borrow()
            .outbound()
            .send(wire::AppToDev::ViewportChanged {
                width: viewport.width,
                height: viewport.height,
            });
        *last = Some(viewport);
    }

    pub fn drain_with_dt_ms(&self, dt_ms: u32) -> bool {
        let mut msgs: Vec<DevToApp> = Vec::new();
        while let Ok(msg) = self.inbound.try_recv() {
            msgs.push(msg);
        }
        let had_inbound = !msgs.is_empty();
        let mut client = self.client.borrow_mut();
        for msg in msgs {
            apply_dev_msg(&mut client, msg);
        }
        // Always send RequestFrame, even when the inbound queue
        // was empty — the sidecar may be mid-animation with no
        // new commands to deliver this exact tick. Skipping the
        // RequestFrame would stall the clock for that frame.
        let _ = client.outbound().send(wire::AppToDev::RequestFrame { dt_ms });
        had_inbound
    }
}

/// Whether a dev-server's reported protocol version is incompatible
/// with the version this client was built against.
///
/// Any difference is incompatible: the wire format is not forward- or
/// backward-compatible across [`wire::PROTOCOL_VERSION`] bumps (a
/// renamed/added field, a new `Command` variant the old replayer
/// can't decode, a changed `Hello` shape, …). The native shell bakes
/// the client at `idealyst dev` time, so the only way to hit a
/// mismatch is to point a stale wrapper binary at a freshly-built
/// dev-server (or vice-versa). We surface it loudly rather than
/// silently failing — see [`apply_dev_msg`].
pub fn protocol_mismatch(server_version: u32) -> bool {
    server_version != wire::PROTOCOL_VERSION
}

/// Decide what to do with one inbound `DevToApp` message. Split out
/// of [`RuntimeServerShell::drain`] so per-message handling stays trivial.
fn apply_dev_msg<B: Backend + 'static>(client: &mut RuntimeServerClient<B>, msg: DevToApp) {
    match msg {
        DevToApp::Hello { session, protocol_version, .. } => {
            // A protocol mismatch means the commands that follow may
            // fail to decode or replay into the wrong backend calls —
            // the classic "blank screen after a wire bump" footgun.
            // The legacy `transport::connect_and_run` path already
            // warned here; the worker-thread shell (iOS / Android /
            // macOS) used to swallow it entirely. Match the legacy
            // behavior so the failure is diagnosable rather than a
            // silent no-render. Rebuilding via `idealyst dev` re-bakes
            // a matching client.
            if protocol_mismatch(protocol_version) {
                eprintln!(
                    "[runtime-server-shell] PROTOCOL VERSION MISMATCH: dev-server speaks \
                     v{protocol_version}, this client was built for v{}. Replay will likely \
                     fail or render incorrectly — rebuild the app via `idealyst dev` to bake \
                     a matching client.",
                    wire::PROTOCOL_VERSION
                );
            }
            if !session.is_empty() {
                eprintln!("[runtime-server-shell] connected to session: {}", session);
            }
        }
        DevToApp::Commands(cmds) => {
            let count = cmds.len();
            // Catch panics from inside apply_batch so a single bad
            // command (e.g. a backend-side objc msg-send type
            // mismatch) doesn't abort the drain loop silently.
            // Without this, AppKit-side panics absorbed by the
            // outer dispatch_async_f catch left "[backend-macos::
            // aas] drain panic absorbed" as the only signal —
            // which made it hard to tell whether *replay* was
            // failing or just the trampoline catch was working.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                client.apply_batch(cmds)
            }));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    eprintln!("[runtime-server-shell] replay error after {count} cmds: {:?}", e);
                }
                Err(payload) => {
                    let msg = if let Some(s) = payload.downcast_ref::<&'static str>() {
                        (*s).to_string()
                    } else if let Some(s) = payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "<non-string panic payload>".to_string()
                    };
                    eprintln!(
                        "[runtime-server-shell] PANIC during apply_batch ({count} cmds): {msg}"
                    );
                }
            }
        }
        DevToApp::Rebuilding => eprintln!("[runtime-server-shell] dev rebuilding…"),
        DevToApp::Error { message } => eprintln!("[runtime-server-shell] dev error: {}", message),
        DevToApp::ThemeChanged { .. } => {}
    }
}

/// Background-thread worker. Connects to the supplied URL, sends a
/// Hello, then pumps frames between socket and channels. On
/// disconnect reconnects to the same URL — works for both the dev-
/// server hot-reloading on a stable port (preserved across host
/// respawns via `IDEALYST_RUNTIME_SERVER_BIND_PORT`) and the rare
/// case where the dev-server moved (manual restart on a new port,
/// which requires the wrapper to be rebuilt anyway).
fn ws_worker_loop(
    url: String,
    inbound_tx: mpsc::Sender<DevToApp>,
    outbound_rx: mpsc::Receiver<AppToDev>,
    options: RuntimeServerShellOptions,
) {
    eprintln!("[runtime-server-shell] worker starting; url={:?}", url);
    loop {
        eprintln!("[runtime-server-shell] connecting to {}", url);
        let (mut ws, _) = match tungstenite::connect(&url) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[runtime-server-shell] connect failed: {} — retrying in 1s", e);
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
            app_name: "runtime-server-client".into(),
            color_scheme: wire::WireColorScheme::Auto,
            initial_url: None,
            identity: wire::ClientIdentity {
                platform: options.platform,
                device_label: options.device_label.clone(),
            },
            viewport: options.viewport.clone(),
        };
        let _ = ws_send(&mut ws, &hello);
        eprintln!("[runtime-server-shell] connected");

        let _ = run_ws_session(&mut ws, &inbound_tx, &outbound_rx);
        eprintln!("[runtime-server-shell] disconnected; reconnecting");
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
                Err(e) => eprintln!("[runtime-server-shell] decode error: {}", e),
            },
            Ok(Message::Text(t)) => match serde_json::from_str::<DevToApp>(t.as_str()) {
                Ok(msg) => {
                    if inbound_tx.send(msg).is_err() {
                        return Ok(());
                    }
                }
                Err(e) => eprintln!("[runtime-server-shell] decode error: {}", e),
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
