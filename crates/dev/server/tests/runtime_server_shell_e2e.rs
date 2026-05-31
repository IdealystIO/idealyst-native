//! End-to-end tests for the **native runtime-server client shell** —
//! the layer that changed when the connection path was unified into
//! `runtime-server-shell-native` (commit `32b524a`).
//!
//! Every other test in this crate stops at the wire: `sessions.rs`
//! drives a raw command-capturing [`MockClient`], and the recorder
//! tests stop at the dev side. None of them exercise the *actual*
//! production client — the [`RuntimeServerShell`] worker thread, its
//! `tungstenite` transport, the `drain()` apply loop, or the outbound
//! `RequestFrame` pump. That is precisely the code an iOS / Android /
//! macOS app runs in hot-reload mode, and it had **zero** coverage.
//!
//! These tests stand up the real loopback dev-server (the same
//! `serve_with_sidecar_and_tracker` + fake-sidecar harness `sessions.rs`
//! uses) and connect a real `RuntimeServerShell` whose backend is a
//! recording [`RecordingBackend`]. The full path under test:
//!
//! ```text
//!   fake sidecar ──Commands──▶ host SceneModel ──WebSocket──▶
//!     RuntimeServerShell worker thread ──mpsc──▶ shell.drain()
//!       ──▶ WireBackend::apply_batch ──▶ RecordingBackend
//! ```
//!
//! plus the reverse channel (`shell.drain()` ships `RequestFrame`
//! back, the host forwards it to the sidecar).
//!
//! What each scenario pins down:
//! - **connect + snapshot replay**: a freshly-spawned shell connects,
//!   completes the Hello exchange, receives the session's scene, and
//!   the tree materializes as real backend calls. Catches "apps don't
//!   connect / don't render anymore."
//! - **RequestFrame pump**: `drain()` ships `RequestFrame` outbound
//!   every tick. Without it the sidecar's animation clock never
//!   advances and the first frame stays blank (the pre-fix native
//!   bug documented in `shell.rs::drain`). Catches a silent stall.
//! - **hot-reload re-snapshot**: a `SessionReset` + fresh scene (what
//!   the sidecar emits after a hot-patch) re-snapshots cleanly into
//!   the already-connected client.
//! - **reconnect / late-joiner catch-up**: a client that connects
//!   *after* state exists is caught up from the `SceneModel` snapshot
//!   — the same server path a reconnecting client hits.
//! - **protocol-version guard**: the predicate behind the shell's
//!   mismatch warning.
//!
//! Run with `cargo test -p dev-server`. `runtime-server-shell-native`
//! is pulled in (with its `runtime-server` feature) via dev-deps.

use std::net::TcpListener;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dev_server::test_support::{emit_from_fake_sidecar, emit_session_reset_then};
use dev_server::{
    serve_with_sidecar_and_tracker, SessionMode, SessionTracker, Sidecar, SidecarIn,
    WireRecordingBackend,
};
use runtime_core::{Backend, StyleRules};
use runtime_server_shell_native::{protocol_mismatch, RuntimeServerShell};
use wire::{Command, NodeId};

// ---------------------------------------------------------------------------
// RecordingBackend — a minimal `Backend` that records the structural
// calls the shell replays into it. This is the stand-in for a real
// platform backend (UIKit / Android View / DOM); it just notes what it
// was told to build so the test can assert the tree arrived.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RecordingBackend {
    next: u64,
    /// Text contents in receipt order — the primary assertion surface.
    texts: Vec<String>,
    view_count: usize,
    finish_count: usize,
}

impl RecordingBackend {
    fn texts(&self) -> Vec<String> {
        self.texts.clone()
    }
}

impl Backend for RecordingBackend {
    type Node = u64;

    fn create_view(&mut self, _a11y: &runtime_core::accessibility::AccessibilityProps) -> u64 {
        self.next += 1;
        self.view_count += 1;
        self.next
    }

    fn create_text(
        &mut self,
        content: &str,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> u64 {
        self.next += 1;
        self.texts.push(content.to_string());
        self.next
    }

    fn create_button(
        &mut self,
        _label: &str,
        _on_click: &runtime_core::Action,
        _leading: Option<&runtime_core::primitives::icon::IconData>,
        _trailing: Option<&runtime_core::primitives::icon::IconData>,
        _a11y: &runtime_core::accessibility::AccessibilityProps,
    ) -> u64 {
        self.next += 1;
        self.next
    }

    fn insert(&mut self, _parent: &mut u64, _child: u64) {}

    fn update_text(&mut self, _node: &u64, content: &str) {
        // Reflect the post-snapshot text so reactive label updates are
        // observable too.
        self.texts.push(content.to_string());
    }

    fn clear_children(&mut self, _node: &u64) {}

    fn apply_style(&mut self, _node: &u64, _style: &Rc<StyleRules>) {}

    fn finish(&mut self, _root: u64) {
        self.finish_count += 1;
    }
}

// ---------------------------------------------------------------------------
// Loopback dev-server harness — mirrors `sessions.rs::spin_up_server`.
// ---------------------------------------------------------------------------

fn pick_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn wait_for_port(addr: &str, total: Duration) {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(addr).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("server at {} never came up within {:?}", addr, total);
}

/// Spin up a `serve_with_sidecar_and_tracker` loop against a fake
/// sidecar under the requested [`SessionMode`]. Returns the ws URL,
/// the fake-sidecar inbound receiver (frames the host sends *to* the
/// sidecar), and the fake-sidecar outbound sender (frames we inject
/// *as if* produced by the author runtime).
fn spin_up_server(
    mode: SessionMode,
) -> (
    String,
    std::sync::mpsc::Receiver<SidecarIn>,
    std::sync::mpsc::Sender<dev_server::SidecarOut>,
) {
    let port = pick_free_port();
    let addr = format!("127.0.0.1:{}", port);
    let url = format!("ws://{}", addr);

    let (sidecar, fake_in_rx, fake_out_tx) = Sidecar::for_test_with_channels();
    let sidecar_slot: dev_server::SidecarSlot = Arc::new(Mutex::new(Some(sidecar)));
    let port_mirror = Arc::new(Mutex::new(None));
    let tracker = SessionTracker::new();

    let addr_for_thread = addr.clone();
    thread::spawn(move || {
        let recorder = WireRecordingBackend::new();
        let _ = serve_with_sidecar_and_tracker(
            addr_for_thread,
            recorder,
            port_mirror,
            sidecar_slot,
            tracker,
            mode,
        );
    });

    wait_for_port(&addr, Duration::from_secs(3));
    (url, fake_in_rx, fake_out_tx)
}

/// One minimal but well-formed scene: View → Text → Insert → Finish.
/// Well-formedness matters — the host mirror's `snapshot()` only emits
/// nodes reachable from the finish-set root, so a bare `CreateText`
/// with no `Insert` would be pruned and never reach the client.
fn marker_scene(text: &str) -> Vec<Command> {
    let root = NodeId(1);
    let txt = NodeId(2);
    vec![
        Command::CreateView {
            id: root,
            a11y: Default::default(),
        },
        Command::CreateText {
            id: txt,
            content: text.to_string(),
            a11y: Default::default(),
        },
        Command::Insert {
            parent: root,
            child: txt,
        },
        Command::Finish { root },
    ]
}

/// Block waiting for `n` `CreateSession` frames so we know the host
/// has registered the shell's session before we emit a scene for it.
fn recv_create_sessions(
    rx: &std::sync::mpsc::Receiver<SidecarIn>,
    n: usize,
    timeout: Duration,
) -> Vec<String> {
    let deadline = Instant::now() + timeout;
    let mut found = Vec::new();
    while found.len() < n && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(SidecarIn::CreateSession { session, .. }) => found.push(session),
            Ok(_) => continue,
            Err(_) => continue,
        }
    }
    found
}

/// Drive the shell's main-thread drain loop until `pred` holds against
/// the recording backend, or the deadline elapses. Each iteration
/// pulls whatever the worker thread has shipped over the channel and
/// applies it, then sleeps briefly so the worker has time to receive
/// the next broadcast frame (the host broadcasts on a ~20ms tick).
fn pump_until<F>(shell: &RuntimeServerShell<RecordingBackend>, timeout: Duration, pred: F) -> bool
where
    F: Fn(&RecordingBackend) -> bool,
{
    let backend = shell.client.borrow().backend().clone();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        shell.drain();
        if pred(&backend.borrow()) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    // One final drain so a frame that landed during the last sleep is
    // applied before we report failure.
    shell.drain();
    let result = pred(&backend.borrow());
    result
}

fn texts_of(shell: &RuntimeServerShell<RecordingBackend>) -> Vec<String> {
    let backend = shell.client.borrow().backend().clone();
    let texts = backend.borrow().texts();
    texts
}

// ---------------------------------------------------------------------------
// 1. Connect + initial snapshot replays into the real backend.
// ---------------------------------------------------------------------------

#[test]
fn shell_connects_and_replays_initial_scene_into_backend() {
    let (url, fake_in_rx, fake_out_tx) = spin_up_server(SessionMode::Shared);

    let shell = RuntimeServerShell::spawn(RecordingBackend::default(), url);

    // The worker connects + Hellos on its own thread; wait until the
    // host has minted the session so our emit lands on a live session.
    let creates = recv_create_sessions(&fake_in_rx, 1, Duration::from_secs(3));
    assert_eq!(creates, vec!["shared".to_string()], "shared session must register");

    emit_from_fake_sidecar(&fake_out_tx, "shared", marker_scene("hello-from-server"));

    let ok = pump_until(&shell, Duration::from_secs(5), |b| {
        b.texts().iter().any(|t| t == "hello-from-server")
    });
    assert!(
        ok,
        "shell never replayed the server's scene into the backend; got texts {:?}. \
         If this fails, the connection / Hello / snapshot / replay path is broken — \
         i.e. apps no longer render in hot-reload mode.",
        texts_of(&shell),
    );

    let backend = shell.client.borrow().backend().clone();
    let b = backend.borrow();
    assert!(b.view_count >= 1, "root View must have been created");
    assert!(b.finish_count >= 1, "Finish must have been replayed");
}

// ---------------------------------------------------------------------------
// 2. drain() pumps RequestFrame back to the server (animation cadence).
// ---------------------------------------------------------------------------

#[test]
fn shell_drain_pumps_request_frame_to_sidecar() {
    let (url, fake_in_rx, _fake_out_tx) = spin_up_server(SessionMode::Shared);

    let shell = RuntimeServerShell::spawn(RecordingBackend::default(), url);
    let _ = recv_create_sessions(&fake_in_rx, 1, Duration::from_secs(3));

    // Drive several drains. Each one ships an `AppToDev::RequestFrame`
    // outbound; in sidecar mode the host forwards every client message
    // (RequestFrame included) to the sidecar as `SidecarIn::Event`.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_request_frame = false;
    while Instant::now() < deadline && !saw_request_frame {
        shell.drain();
        // Drain whatever the host has forwarded to the fake sidecar so
        // far, looking for a RequestFrame.
        while let Ok(frame) = fake_in_rx.recv_timeout(Duration::from_millis(50)) {
            if let SidecarIn::Event {
                event: wire::AppToDev::RequestFrame { .. },
                ..
            } = frame
            {
                saw_request_frame = true;
                break;
            }
        }
    }

    assert!(
        saw_request_frame,
        "shell.drain() must ship AppToDev::RequestFrame so the sidecar's animation \
         clock advances. Without it, timeline tweens never start and the first frame \
         stays blank (the pre-fix native runtime-server bug)."
    );
}

// ---------------------------------------------------------------------------
// 3. Hot-reload: SessionReset + fresh scene re-snapshots into the
//    already-connected client.
// ---------------------------------------------------------------------------

#[test]
fn shell_hot_reload_resnapshot_rebuilds_connected_backend() {
    let (url, fake_in_rx, fake_out_tx) = spin_up_server(SessionMode::Shared);

    let shell = RuntimeServerShell::spawn(RecordingBackend::default(), url);
    let _ = recv_create_sessions(&fake_in_rx, 1, Duration::from_secs(3));

    // Initial render.
    emit_from_fake_sidecar(&fake_out_tx, "shared", marker_scene("before-patch"));
    assert!(
        pump_until(&shell, Duration::from_secs(5), |b| b
            .texts()
            .iter()
            .any(|t| t == "before-patch")),
        "client must see the pre-patch scene; got {:?}",
        texts_of(&shell),
    );

    // Hot-patch lands: the sidecar tears down + re-renders, emitting a
    // SessionReset (host drops its mirror log + bumps epoch) followed
    // by the fresh scene. The connected client must pick up the new
    // content without a reconnect.
    emit_session_reset_then(&fake_out_tx, "shared", marker_scene("after-patch"));
    assert!(
        pump_until(&shell, Duration::from_secs(5), |b| b
            .texts()
            .iter()
            .any(|t| t == "after-patch")),
        "client must replay the post-hot-patch re-snapshot; got {:?}. \
         If this fails, hot reload no longer reaches already-connected clients.",
        texts_of(&shell),
    );
}

// ---------------------------------------------------------------------------
// 4. Reconnect / late-joiner catch-up from the SceneModel snapshot.
//    A reconnecting client sends a fresh Hello and is caught up the
//    same way a never-before-seen client is — this exercises that
//    server snapshot path from the real shell's perspective.
// ---------------------------------------------------------------------------

#[test]
fn shell_joining_after_state_exists_is_caught_up_via_snapshot() {
    let (url, fake_in_rx, fake_out_tx) = spin_up_server(SessionMode::Shared);

    // First client establishes the shared scene.
    let early = RuntimeServerShell::spawn(RecordingBackend::default(), url.clone());
    let _ = recv_create_sessions(&fake_in_rx, 1, Duration::from_secs(3));
    emit_from_fake_sidecar(&fake_out_tx, "shared", marker_scene("already-there"));
    assert!(
        pump_until(&early, Duration::from_secs(5), |b| b
            .texts()
            .iter()
            .any(|t| t == "already-there")),
        "early client must see the scene before the late joiner connects",
    );

    // A second shell connects after the fact — the same thing that
    // happens when a client drops and reconnects. It must be caught up
    // from the host's SceneModel snapshot, not left blank.
    let late = RuntimeServerShell::spawn(RecordingBackend::default(), url);
    assert!(
        pump_until(&late, Duration::from_secs(5), |b| b
            .texts()
            .iter()
            .any(|t| t == "already-there")),
        "late-joining / reconnecting client must be caught up via snapshot; got {:?}. \
         If this fails, a client that reconnects after a drop renders nothing.",
        texts_of(&late),
    );
}

// ---------------------------------------------------------------------------
// 5. Protocol-version mismatch predicate (the guard behind the shell's
//    loud warning). Regression test for the silent-version-skew gap
//    in `shell.rs::apply_dev_msg`.
// ---------------------------------------------------------------------------

#[test]
fn protocol_mismatch_predicate_flags_only_skew() {
    assert!(
        !protocol_mismatch(wire::PROTOCOL_VERSION),
        "the version this client was built for must never be a mismatch"
    );
    assert!(
        protocol_mismatch(wire::PROTOCOL_VERSION + 1),
        "a newer dev-server protocol must be flagged as a mismatch"
    );
    assert!(
        protocol_mismatch(wire::PROTOCOL_VERSION.wrapping_sub(1)),
        "an older dev-server protocol must be flagged as a mismatch"
    );
}
