//! Multi-session integration tests for the dev-server transport.
//!
//! Drives the real `serve_with_sidecar_and_tracker` loop against a
//! **fake** sidecar (constructed via `Sidecar::for_test_with_channels`)
//! so we can assert what the host says to the sidecar without
//! compiling + launching the generated `<project>-aas-app` binary.
//!
//! Sessions are entirely server-side: clients never name one. They
//! only declare an identity (platform + optional device label) used
//! for logging. The two server modes:
//!
//! - [`SessionMode::PerClient`] (default): every connection gets a
//!   unique server-minted id and its own session in the sidecar.
//! - [`SessionMode::Shared`]: every connection lands on the same
//!   well-known `"shared"` session (the legacy synced-devices mode).
//!
//! Each test runs the server on a worker thread, drives clients from
//! the main thread, and inspects what the fake sidecar received via
//! the `fake_in_rx` channel returned alongside the test sidecar.

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dev_server::{
    serve_with_sidecar_and_tracker, SessionMode, SessionTracker, Sidecar, SidecarIn, SidecarOut,
    WireRecordingBackend,
};
use tungstenite::Message;
use wire::{AppToDev, ClientIdentity, DevToApp, WirePlatform};

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
    std::sync::mpsc::Sender<SidecarOut>,
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

fn connect_with_identity(
    url: &str,
    platform: WirePlatform,
    label: Option<&str>,
) -> tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>> {
    let (mut ws, _) = tungstenite::connect(url).expect("connect");
    if let tungstenite::stream::MaybeTlsStream::Plain(s) = ws.get_ref() {
        let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    }
    let hello = AppToDev::Hello {
        app_name: "test".into(),
        color_scheme: wire::WireColorScheme::Auto,
        initial_url: None,
        identity: ClientIdentity {
            platform,
            device_label: label.map(str::to_string),
        },
    };
    ws.send(Message::Binary(serde_json::to_vec(&hello).unwrap().into()))
        .unwrap();
    ws
}

fn read_one_msg(
    ws: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
) -> DevToApp {
    loop {
        match ws.read().expect("read") {
            Message::Binary(b) => return serde_json::from_slice(&b).expect("decode"),
            Message::Text(t) => return serde_json::from_str(&t).expect("decode"),
            Message::Ping(p) => {
                let _ = ws.send(Message::Pong(p));
            }
            _ => continue,
        }
    }
}

fn recv_create_sessions(
    rx: &std::sync::mpsc::Receiver<SidecarIn>,
    expected_count: usize,
    timeout: Duration,
) -> Vec<String> {
    let deadline = Instant::now() + timeout;
    let mut found = Vec::new();
    while found.len() < expected_count && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(SidecarIn::CreateSession { session }) => found.push(session),
            Ok(_) => continue, // other ins (events) — ignore
            Err(_) => continue,
        }
    }
    found
}

#[test]
fn per_client_mode_each_connection_gets_a_fresh_session() {
    let (url, fake_in_rx, _fake_out_tx) = spin_up_server(SessionMode::PerClient);

    let mut ws_a = connect_with_identity(&url, WirePlatform::Web, Some("Chrome"));
    let mut ws_b = connect_with_identity(&url, WirePlatform::Ios, Some("iPhone 15"));

    let hello_a = match read_one_msg(&mut ws_a) {
        DevToApp::Hello { session, .. } => session,
        other => panic!("expected Hello, got {:?}", other),
    };
    let hello_b = match read_one_msg(&mut ws_b) {
        DevToApp::Hello { session, .. } => session,
        other => panic!("expected Hello, got {:?}", other),
    };

    assert_ne!(
        hello_a, hello_b,
        "PerClient mode must assign distinct sessions per connection"
    );
    assert!(
        hello_a.starts_with("web_"),
        "web client should land on a web_-prefixed id, got {}",
        hello_a
    );
    assert!(
        hello_b.starts_with("ios_"),
        "iOS client should land on an ios_-prefixed id, got {}",
        hello_b
    );

    let creates = recv_create_sessions(&fake_in_rx, 2, Duration::from_secs(2));
    assert_eq!(
        creates.len(),
        2,
        "host should issue exactly two CreateSession frames"
    );
    assert!(creates.contains(&hello_a), "missing CreateSession for {}", hello_a);
    assert!(creates.contains(&hello_b), "missing CreateSession for {}", hello_b);
}

#[test]
fn shared_mode_all_clients_land_on_one_session() {
    let (url, fake_in_rx, _fake_out_tx) = spin_up_server(SessionMode::Shared);

    let mut ws_a = connect_with_identity(&url, WirePlatform::Web, None);
    let hello_a = match read_one_msg(&mut ws_a) {
        DevToApp::Hello { session, .. } => session,
        other => panic!("expected Hello, got {:?}", other),
    };
    assert_eq!(hello_a, "shared");

    let mut ws_b = connect_with_identity(&url, WirePlatform::Ios, None);
    let hello_b = match read_one_msg(&mut ws_b) {
        DevToApp::Hello { session, .. } => session,
        other => panic!("expected Hello, got {:?}", other),
    };
    assert_eq!(hello_b, "shared");

    // Only the first client triggers CreateSession; the second
    // attaches to the existing slot.
    let mut creates = 0;
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        match fake_in_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(SidecarIn::CreateSession { session }) => {
                assert_eq!(session, "shared");
                creates += 1;
            }
            Ok(_) => {}
            Err(_) => {}
        }
    }
    assert_eq!(
        creates, 1,
        "second client must attach silently to the shared session"
    );
}

#[test]
fn server_minted_id_format_is_platform_prefixed_hex_suffix() {
    let (url, _fake_in_rx, _fake_out_tx) = spin_up_server(SessionMode::PerClient);
    let mut ws = connect_with_identity(&url, WirePlatform::Android, Some("Pixel 8"));
    let hello = match read_one_msg(&mut ws) {
        DevToApp::Hello { session, .. } => session,
        other => panic!("expected Hello, got {:?}", other),
    };
    let suffix = hello
        .strip_prefix("android_")
        .unwrap_or_else(|| panic!("expected android_ prefix, got {}", hello));
    assert_eq!(suffix.len(), 8);
    assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn event_from_client_is_routed_with_its_session_id() {
    let (url, fake_in_rx, _fake_out_tx) = spin_up_server(SessionMode::PerClient);

    let mut ws = connect_with_identity(&url, WirePlatform::Web, Some("test-client"));
    let assigned = match read_one_msg(&mut ws) {
        DevToApp::Hello { session, .. } => session,
        other => panic!("expected Hello, got {:?}", other),
    };

    // Drain the CreateSession that fired on the handshake.
    let _ = recv_create_sessions(&fake_in_rx, 1, Duration::from_secs(1));

    // Fire an event from the client side. The host should forward it
    // to the sidecar tagged with `assigned`.
    let event = AppToDev::Event {
        handler: wire::HandlerId(42),
        args: wire::EventArgs::Unit,
    };
    ws.send(Message::Binary(serde_json::to_vec(&event).unwrap().into()))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut got: Option<(String, wire::HandlerId)> = None;
    while got.is_none() && Instant::now() < deadline {
        if let Ok(msg) = fake_in_rx.recv_timeout(Duration::from_millis(100)) {
            if let SidecarIn::Event {
                session,
                event: AppToDev::Event { handler, .. },
            } = msg
            {
                got = Some((session, handler));
                break;
            }
        }
    }
    let (session, handler) = got.expect("event was never forwarded to the fake sidecar");
    assert_eq!(session, assigned);
    assert_eq!(handler.0, 42);
}
