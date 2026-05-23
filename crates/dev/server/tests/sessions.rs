//! End-to-end multi-session tests for the dev-server transport.
//!
//! Each test stands up a real `serve_with_sidecar_and_tracker` loop
//! on a loopback port, connects N `MockClient`s over real
//! WebSockets, and drives the (fake) sidecar to emit commands and
//! reset events. The host's behavior is exercised through the same
//! code paths a production sidecar / real device would hit; only the
//! two ends (sidecar process, platform binary) are stubbed.
//!
//! Coverage:
//!
//! - PerClient mode hands out distinct platform-prefixed session ids
//!   and emits one `CreateSession` per client to the sidecar.
//! - Shared mode pins every client to `"shared"` and emits exactly
//!   one `CreateSession`.
//! - **Isolation**: commands emitted by the sidecar for session A
//!   reach only client A in PerClient mode.
//! - **Fan-out**: commands emitted for the shared session reach
//!   every connected client.
//! - **Reverse channel**: an event fired by a client is forwarded to
//!   the sidecar tagged with that client's session id.
//! - **Hot-patch re-snapshot**: a `SessionReset` followed by a fresh
//!   command batch triggers a re-snapshot on the attached clients
//!   (their epoch advances + cursor resets).
//! - **Late joiner**: a client connecting after the sidecar has
//!   already shipped commands gets a fresh snapshot.

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dev_server::test_support::{emit_from_fake_sidecar, emit_session_reset_then, MockClient};
use dev_server::{
    serve_with_sidecar_and_tracker, SessionMode, SessionTracker, Sidecar, SidecarIn,
    WireRecordingBackend,
};
use wire::{ClientIdentity, Command, EventArgs, HandlerId, NodeId, WirePlatform};

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
/// sidecar under the requested [`SessionMode`].
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
            "session-test",
            port_mirror,
            sidecar_slot,
            tracker,
            mode,
        );
    });

    wait_for_port(&addr, Duration::from_secs(3));
    (url, fake_in_rx, fake_out_tx)
}

fn identity(platform: WirePlatform, label: &str) -> ClientIdentity {
    ClientIdentity {
        platform,
        device_label: Some(label.to_string()),
    }
}

/// Drain `n` `CreateSession` frames from the fake sidecar's inbound
/// channel, with a deadline. Other frame kinds are dropped.
fn recv_n_create_sessions(
    rx: &std::sync::mpsc::Receiver<SidecarIn>,
    n: usize,
    timeout: Duration,
) -> Vec<String> {
    let deadline = Instant::now() + timeout;
    let mut found = Vec::new();
    while found.len() < n && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(SidecarIn::CreateSession { session }) => found.push(session),
            Ok(_) => continue,
            Err(_) => continue,
        }
    }
    found
}

/// One minimal but well-formed scene: a View root containing a Text,
/// terminated by `Finish`. Well-formedness matters because the host
/// mirror's `snapshot()` only emits nodes reachable from the
/// finish-set root — a bare `CreateText` with no `Insert` would be
/// pruned by `compute_reachable`, breaking late-joiner / post-reset
/// snapshot tests.
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

/// Pull every `CreateText.content` out of the captured command
/// stream, in order. Used by tests to assert which text payloads a
/// client actually saw.
fn texts(cmds: &[Command]) -> Vec<&str> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::CreateText { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Session-assignment shape
// ---------------------------------------------------------------------------

#[test]
fn per_client_mode_each_connection_gets_a_fresh_session() {
    let (url, fake_in_rx, _fake_out_tx) = spin_up_server(SessionMode::PerClient);

    let a = MockClient::connect(&url, identity(WirePlatform::Web, "Chrome"));
    let b = MockClient::connect(&url, identity(WirePlatform::Ios, "iPhone 15"));

    assert_ne!(a.assigned_session, b.assigned_session);
    assert!(
        a.assigned_session.starts_with("web_"),
        "web client should land on web_-prefixed id, got {}",
        a.assigned_session
    );
    assert!(
        b.assigned_session.starts_with("ios_"),
        "iOS client should land on ios_-prefixed id, got {}",
        b.assigned_session
    );

    let creates = recv_n_create_sessions(&fake_in_rx, 2, Duration::from_secs(2));
    assert_eq!(creates.len(), 2);
    assert!(creates.contains(&a.assigned_session));
    assert!(creates.contains(&b.assigned_session));
}

#[test]
fn shared_mode_all_clients_land_on_one_session() {
    let (url, fake_in_rx, _fake_out_tx) = spin_up_server(SessionMode::Shared);

    let a = MockClient::connect(&url, identity(WirePlatform::Web, "A"));
    let b = MockClient::connect(&url, identity(WirePlatform::Ios, "B"));

    assert_eq!(a.assigned_session, "shared");
    assert_eq!(b.assigned_session, "shared");

    // Only the first client should trigger CreateSession.
    let mut creates = 0;
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if let Ok(SidecarIn::CreateSession { session }) =
            fake_in_rx.recv_timeout(Duration::from_millis(100))
        {
            assert_eq!(session, "shared");
            creates += 1;
        }
    }
    assert_eq!(creates, 1, "second client must attach silently to shared");
}

#[test]
fn server_minted_id_format_is_platform_prefixed_hex_suffix() {
    let (url, _fake_in_rx, _fake_out_tx) = spin_up_server(SessionMode::PerClient);
    let c = MockClient::connect(&url, identity(WirePlatform::Android, "Pixel 8"));
    let suffix = c
        .assigned_session
        .strip_prefix("android_")
        .unwrap_or_else(|| panic!("expected android_ prefix, got {}", c.assigned_session));
    assert_eq!(suffix.len(), 8);
    assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
}

// ---------------------------------------------------------------------------
// Fan-out + isolation (the load-bearing tests)
// ---------------------------------------------------------------------------

#[test]
fn per_client_mode_commands_for_session_a_do_not_reach_client_b() {
    let (url, fake_in_rx, fake_out_tx) = spin_up_server(SessionMode::PerClient);

    let mut a = MockClient::connect(&url, identity(WirePlatform::Web, "A"));
    let mut b = MockClient::connect(&url, identity(WirePlatform::Web, "B"));

    let session_a = a.assigned_session.clone();
    let _session_b = b.assigned_session.clone();

    // Wait for both CreateSession frames so we know the host has both
    // sessions registered before we emit.
    let creates = recv_n_create_sessions(&fake_in_rx, 2, Duration::from_secs(2));
    assert_eq!(creates.len(), 2);

    // Sidecar emits a small scene for session A only.
    let a_scene = marker_scene("A-only");
    emit_from_fake_sidecar(&fake_out_tx, &session_a, a_scene.clone());

    // A must see every command in the scene (delta path; no snapshot
    // necessary because A's cursor advances atomically).
    a.expect_at_least(a_scene.len(), Duration::from_secs(2));
    assert_eq!(texts(&a.commands), vec!["A-only"]);

    // B must not see any of it within a generous window.
    b.drain_for(Duration::from_millis(500));
    assert_eq!(
        b.commands.len(),
        0,
        "client B leaked commands from session A: {:?}",
        b.commands
    );
}

#[test]
fn shared_mode_commands_fan_out_to_every_attached_client() {
    let (url, _fake_in_rx, fake_out_tx) = spin_up_server(SessionMode::Shared);

    let mut a = MockClient::connect(&url, identity(WirePlatform::Web, "A"));
    let mut b = MockClient::connect(&url, identity(WirePlatform::Ios, "B"));

    // Both should be on "shared" — verified elsewhere; double-check
    // here so a failure points at the right test.
    assert_eq!(a.assigned_session, "shared");
    assert_eq!(b.assigned_session, "shared");

    // Sidecar emits one shared scene.
    let scene = marker_scene("for-everyone");
    emit_from_fake_sidecar(&fake_out_tx, "shared", scene.clone());

    a.expect_at_least(scene.len(), Duration::from_secs(2));
    b.expect_at_least(scene.len(), Duration::from_secs(2));

    assert_eq!(texts(&a.commands), vec!["for-everyone"]);
    assert_eq!(texts(&b.commands), vec!["for-everyone"]);
}

// ---------------------------------------------------------------------------
// Reverse channel
// ---------------------------------------------------------------------------

#[test]
fn event_from_client_is_routed_to_sidecar_with_its_session_id() {
    let (url, fake_in_rx, _fake_out_tx) = spin_up_server(SessionMode::PerClient);

    let mut a = MockClient::connect(&url, identity(WirePlatform::Web, "A"));
    let session_a = a.assigned_session.clone();
    let _ = recv_n_create_sessions(&fake_in_rx, 1, Duration::from_secs(1));

    a.send_event(HandlerId(7), EventArgs::Unit);

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut got: Option<(String, HandlerId)> = None;
    while got.is_none() && Instant::now() < deadline {
        if let Ok(msg) = fake_in_rx.recv_timeout(Duration::from_millis(100)) {
            if let SidecarIn::Event {
                session,
                event: wire::AppToDev::Event { handler, .. },
            } = msg
            {
                got = Some((session, handler));
                break;
            }
        }
    }
    let (session, handler) = got.expect("event never forwarded to fake sidecar");
    assert_eq!(session, session_a);
    assert_eq!(handler.0, 7);
}

// ---------------------------------------------------------------------------
// Hot-patch reset
// ---------------------------------------------------------------------------

#[test]
fn session_reset_followed_by_commands_triggers_resnap_on_attached_clients() {
    let (url, fake_in_rx, fake_out_tx) = spin_up_server(SessionMode::PerClient);

    let mut a = MockClient::connect(&url, identity(WirePlatform::Web, "A"));
    let session_a = a.assigned_session.clone();
    let _ = recv_n_create_sessions(&fake_in_rx, 1, Duration::from_secs(1));

    // Initial render: ship a small "before" tree (delta path on a
    // fresh-with-empty-snapshot client).
    let before = marker_scene("before");
    emit_from_fake_sidecar(&fake_out_tx, &session_a, before.clone());
    a.expect_at_least(before.len(), Duration::from_secs(2));
    assert_eq!(texts(&a.commands), vec!["before"]);

    // Hot-patch lands: sidecar emits SessionReset followed by a
    // fresh "after" scene. The host must drop its mirror's log, bump
    // the epoch, and ship a fresh snapshot containing the new scene.
    let before_count = a.commands.len();
    let after = marker_scene("after");
    emit_session_reset_then(&fake_out_tx, &session_a, after.clone());

    // After the reset we expect *at least one more* CreateText
    // ("after"). We don't pin the exact total because the resnap
    // ships a full scene (CreateView + CreateText + Insert + Finish),
    // not just a delta of the change.
    a.expect_at_least(before_count + 1, Duration::from_secs(2));
    assert_eq!(
        texts(&a.commands),
        vec!["before", "after"],
        "client should have seen the pre-patch render then the fresh post-patch snapshot",
    );
}

// ---------------------------------------------------------------------------
// Large-frame regression: send must tolerate non-blocking WouldBlock
// ---------------------------------------------------------------------------
//
// Reproduced by the manual scaffolded `welcome` probe: a real initial
// render produces a ~tens-of-KB JSON frame that the kernel can't
// absorb in one non-blocking write. Before the fix, tungstenite's
// WouldBlock error bubbled out of `send()` and the host dropped the
// client. Now the frame is left buffered inside tungstenite and the
// next tick's `flush_best_effort` finishes pushing it.

#[test]
fn large_initial_render_does_not_drop_client_on_wouldblock() {
    let (url, fake_in_rx, fake_out_tx) = spin_up_server(SessionMode::PerClient);

    let mut a = MockClient::connect(&url, identity(WirePlatform::Web, "big"));
    let session = a.assigned_session.clone();
    let _ = recv_n_create_sessions(&fake_in_rx, 1, Duration::from_secs(1));

    // Synthesize a "fat" scene: one root View with N text children.
    // Each `CreateText { content: "...padded..." }` is a few hundred
    // bytes; 800 children → ~50KB JSON frame, easily past the
    // typical 16KB SO_SNDBUF default on macOS loopback. The exact
    // threshold doesn't matter — we just need a frame big enough
    // that a single non-blocking write probably can't absorb it.
    const CHILDREN: u64 = 800;
    let root = NodeId(1);
    let mut cmds = Vec::with_capacity(2 + CHILDREN as usize * 2 + 1);
    cmds.push(Command::CreateView {
        id: root,
        a11y: Default::default(),
    });
    for i in 0..CHILDREN {
        let txt = NodeId(2 + i);
        cmds.push(Command::CreateText {
            id: txt,
            content: format!(
                "child-{:04}-padding-padding-padding-padding-padding-padding",
                i
            ),
            a11y: Default::default(),
        });
        cmds.push(Command::Insert {
            parent: root,
            child: txt,
        });
    }
    cmds.push(Command::Finish { root });
    let expected_text_count = CHILDREN as usize;
    emit_from_fake_sidecar(&fake_out_tx, &session, cmds);

    // The client must receive every CreateText, even though the
    // initial broadcast write almost certainly hits WouldBlock. The
    // generous deadline lets the per-tick flush drain the kernel
    // buffer.
    a.expect_at_least(expected_text_count, Duration::from_secs(5));
    let received_texts = texts(&a.commands);
    assert_eq!(
        received_texts.len(),
        expected_text_count,
        "client must see every text child despite the fat frame triggering WouldBlock"
    );
}

// ---------------------------------------------------------------------------
// Late joiner — caught-up snapshot
// ---------------------------------------------------------------------------

#[test]
fn shared_mode_late_joiner_gets_caught_up_with_existing_commands() {
    let (url, fake_in_rx, fake_out_tx) = spin_up_server(SessionMode::Shared);

    // First client opens the shared session.
    let mut early = MockClient::connect(&url, identity(WirePlatform::Web, "early"));
    let _ = recv_n_create_sessions(&fake_in_rx, 1, Duration::from_secs(1));

    // Sidecar emits some initial state for the shared session.
    let scene = marker_scene("pre-existing");
    emit_from_fake_sidecar(&fake_out_tx, "shared", scene.clone());
    early.expect_at_least(scene.len(), Duration::from_secs(2));

    // Second client connects after the fact — should receive a
    // snapshot rebuilt from the current scene, which includes the
    // pre-existing text.
    let mut late = MockClient::connect(&url, identity(WirePlatform::Ios, "late"));
    late.expect_at_least(1, Duration::from_secs(2));
    assert_eq!(texts(&late.commands), vec!["pre-existing"]);
}
