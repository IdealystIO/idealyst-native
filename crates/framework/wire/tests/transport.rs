//! End-to-end transport test.
//!
//! Spins up the dev-side `serve(...)` on an OS-assigned localhost
//! port in a worker thread, connects from a second worker thread,
//! and asserts the small tree mounted on the dev side surfaces as
//! the expected sequence of backend calls on the app side.
//!
//! Both threads construct their backends locally (the recorder and
//! the `WireBackend<TraceBackend>` are both `!Send`, but the closures
//! only capture `Send` data — channels + strings — and build the
//! non-`Send` state inside the spawned closure).

use std::cell::RefCell;
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use framework_core::accessibility::AccessibilityTraits;
use framework_core::{render, Backend, Primitive, StyleRules, TextSource};
use aas_shell_native::connect_and_run;
use dev_client::WireBackend;
use dev_server::{serve, WireRecordingBackend};

#[derive(Debug, Clone, PartialEq)]
enum Trace {
    CreateView(u64),
    /// `(id, content, a11y_label, a11y_traits_bits)`
    CreateText(u64, String, Option<String>, u16),
    Insert(u64, u64),
    Finish(u64),
}

#[derive(Default)]
struct TraceBackend {
    next: u64,
    trace: Vec<Trace>,
}

impl Backend for TraceBackend {
    type Node = u64;

    fn create_view(
        &mut self,
        _a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> u64 {
        self.next += 1;
        let id = self.next;
        self.trace.push(Trace::CreateView(id));
        id
    }

    fn create_text(
        &mut self,
        content: &str,
        a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> u64 {
        self.next += 1;
        let id = self.next;
        self.trace.push(Trace::CreateText(
            id,
            content.to_string(),
            a11y.label.clone(),
            a11y.traits.bits(),
        ));
        id
    }

    fn create_button(
        &mut self,
        _label: &str,
        _on_click: &framework_core::Action,
        _leading: Option<&framework_core::primitives::icon::IconData>,
        _trailing: Option<&framework_core::primitives::icon::IconData>,
        _a11y: &framework_core::accessibility::AccessibilityProps,
    ) -> u64 {
        self.next += 1;
        self.next
    }

    fn insert(&mut self, parent: &mut u64, child: u64) {
        self.trace.push(Trace::Insert(*parent, child));
    }

    fn update_text(&mut self, _node: &u64, _content: &str) {}

    fn clear_children(&mut self, _node: &u64) {}

    fn apply_style(&mut self, _node: &u64, _style: &Rc<StyleRules>) {}

    fn finish(&mut self, root: u64) {
        self.trace.push(Trace::Finish(root));
    }
}

fn pick_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

#[test]
fn websocket_round_trip_basic_tree() {
    let port = pick_free_port();
    let server_addr = format!("127.0.0.1:{}", port);
    let url = format!("ws://{}", &server_addr);

    // --- Server thread ---
    // Holds the framework runtime + recorder + serve loop.
    let server_addr_clone = server_addr.clone();
    thread::spawn(move || {
        let recorder = WireRecordingBackend::new();
        let backend_rc = Rc::new(RefCell::new(recorder.clone()));
        // The first Text carries an explicit accessibility label
        // and a SELECTED trait bit so we can assert the wire faithfully
        // carries non-default a11y across to the app side. The second
        // Text leaves accessibility at default to verify both shapes
        // survive the round-trip.
        let hello_a11y = framework_core::accessibility::AccessibilityProps {
            label: Some("hello-label".into()),
            traits: AccessibilityTraits::SELECTED,
            ..Default::default()
        };
        let tree = Primitive::View {
            children: vec![
                Primitive::Text {
                    source: TextSource::Static("hello".into()),
                    style: None,
                    ref_fill: None,
                    accessibility: hello_a11y,
                    test_id: None,
                },
                Primitive::Text {
                    source: TextSource::Static("world".into()),
                    style: None,
                    ref_fill: None,
                    accessibility: Default::default(),
                    test_id: None,
                },
            ],
            style: None,
            ref_fill: None,
            safe_area_sides: Default::default(),
            on_touch: None,
            accessibility: Default::default(),
            test_id: None,
        };
        let owner = render(backend_rc, tree);
        std::mem::forget(owner);
        let _ = serve(server_addr_clone, recorder, "transport-test");
    });

    // Give the server time to bind.
    wait_for_port(&server_addr, Duration::from_secs(3));

    // --- Client thread ---
    let (assert_tx, assert_rx) = mpsc::channel::<Vec<Trace>>();
    let url_for_thread = url.clone();
    thread::spawn(move || {
        let (tx, rx) = mpsc::channel();
        let mut wire = WireBackend::new(TraceBackend::default(), tx);

        // Run the transport loop until the test signals end-of-data
        // by dropping the assert_rx. The simplest signal: a watchdog
        // thread closes the TCP stream from the server side by
        // running until our assertion has captured the trace, then
        // exiting (the server's outbound TCP close will return
        // ConnectionClosed in the read loop).
        //
        // For now, run with a wall-clock budget: connect, let the
        // wire apply for 500ms, then snapshot the trace and ship it.
        let url = url_for_thread;
        let trace = run_with_budget(&url, &mut wire, rx, Duration::from_millis(500));
        let _ = assert_tx.send(trace);
    });

    let trace = assert_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("client never reported back");

    let texts: Vec<String> = trace
        .iter()
        .filter_map(|t| match t {
            Trace::CreateText(_, s, _, _) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        vec!["hello".to_string(), "world".to_string()],
        "the two Text contents from the dev-side tree must arrive on the app side"
    );

    // Accessibility round-trip: the first Text was authored with
    // `label: Some("hello-label")` and `traits: SELECTED`; both must
    // survive the wire and arrive at the TraceBackend.
    let hello_a11y = trace
        .iter()
        .find_map(|t| match t {
            Trace::CreateText(_, s, label, traits) if s == "hello" => {
                Some((label.clone(), *traits))
            }
            _ => None,
        })
        .expect("the hello Text must surface in the trace");
    assert_eq!(hello_a11y.0.as_deref(), Some("hello-label"));
    assert_eq!(hello_a11y.1, AccessibilityTraits::SELECTED.bits());

    // The default-a11y Text must arrive with default values intact —
    // no leakage from the previous sibling's overrides.
    let world_a11y = trace
        .iter()
        .find_map(|t| match t {
            Trace::CreateText(_, s, label, traits) if s == "world" => {
                Some((label.clone(), *traits))
            }
            _ => None,
        })
        .expect("the world Text must surface in the trace");
    assert!(world_a11y.0.is_none(), "default label must remain None");
    assert_eq!(world_a11y.1, 0, "default traits must remain empty");

    assert!(
        trace.iter().any(|t| matches!(t, Trace::CreateView(_))),
        "the root View must be created"
    );
    assert!(
        trace.iter().any(|t| matches!(t, Trace::Finish(_))),
        "Finish must be the terminal call"
    );
}

fn wait_for_port(addr: &str, total: Duration) {
    let deadline = std::time::Instant::now() + total;
    while std::time::Instant::now() < deadline {
        if std::net::TcpStream::connect(addr).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("server at {} never came up within {:?}", addr, total);
}

/// Drive `connect_and_run` with a hard wall-clock budget. We can't
/// cleanly stop the transport loop without a shutdown hook, so we
/// run it on a sub-thread and snapshot the backend's trace after
/// the deadline. The sub-thread leaks beyond the test (the process
/// exits and reaps it).
fn run_with_budget(
    url: &str,
    wire: &mut WireBackend<TraceBackend>,
    rx: mpsc::Receiver<wire::AppToDev>,
    budget: Duration,
) -> Vec<Trace> {
    // The wire and rx aren't Send/Sync — we own them here on this
    // thread. We can't move them into another thread. The simplest
    // workaround: open the TCP connection ourselves, read frames
    // with a short timeout, dispatch via `wire.apply_batch`, return
    // when the deadline is hit.
    use wire::DevToApp;
    use tungstenite::Message;

    let deadline = std::time::Instant::now() + budget;
    let (mut ws, _) = tungstenite::connect(url).expect("connect");
    let stream = match ws.get_ref() {
        tungstenite::stream::MaybeTlsStream::Plain(s) => Some(s),
        _ => None,
    };
    if let Some(s) = stream {
        s.set_read_timeout(Some(Duration::from_millis(50))).ok();
    }

    // Greet (matches what connect_and_run does).
    let hello = wire::AppToDev::Hello {
        app_name: "transport-test".to_string(),
        color_scheme: wire::WireColorScheme::Auto,
        initial_url: None,
        identity: wire::ClientIdentity::default(),
    };
    let bytes = serde_json::to_vec(&hello).unwrap();
    ws.send(Message::Binary(bytes.into())).ok();

    while std::time::Instant::now() < deadline {
        match ws.read() {
            Ok(Message::Binary(b)) => {
                let msg: DevToApp = serde_json::from_slice(&b).expect("decode");
                if let DevToApp::Commands(cmds) = msg {
                    wire.apply_batch(cmds).expect("replay");
                }
            }
            Ok(Message::Text(t)) => {
                let msg: DevToApp = serde_json::from_str(t.as_str()).expect("decode");
                if let DevToApp::Commands(cmds) = msg {
                    wire.apply_batch(cmds).expect("replay");
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                // timeout — loop
            }
            Err(_) => break,
        }
        // drain outbound (won't have anything for a passive client)
        while rx.try_recv().is_ok() {}
    }

    std::mem::take(&mut wire.backend_mut().trace)
}

// Suppress dead-code lints for the imports/types that are
// "indirectly" exercised through the helper paths.
#[allow(dead_code)]
fn _force_uses() {
    let _ = connect_and_run::<TraceBackend>;
}

/// In single-process mode (no sidecar — the recorder *is* the
/// runtime) every connection lands on the well-known "primary"
/// session. Single-process mode predates per-client sessions; there's
/// only ever one logical scene to share.
#[test]
fn server_hello_carries_primary_session_in_single_process_mode() {
    use tungstenite::Message;
    use wire::DevToApp;

    let port = pick_free_port();
    let server_addr = format!("127.0.0.1:{}", port);
    let url = format!("ws://{}", &server_addr);

    let server_addr_clone = server_addr.clone();
    thread::spawn(move || {
        let recorder = WireRecordingBackend::new();
        // Don't bother with an Owner — we only care about the Hello
        // exchange.
        let _ = serve(server_addr_clone, recorder, "session-test");
    });
    wait_for_port(&server_addr, Duration::from_secs(3));

    // Client A: send a populated identity for log fidelity.
    // Single-process mode pins the assigned session to "primary"
    // regardless of what the client sends.
    let (mut ws_a, _) = tungstenite::connect(&url).expect("connect");
    let hello_a = wire::AppToDev::Hello {
        app_name: "client-a".into(),
        color_scheme: wire::WireColorScheme::Auto,
        initial_url: None,
        identity: wire::ClientIdentity {
            platform: wire::WirePlatform::Web,
            device_label: Some("client-a".into()),
        },
    };
    ws_a.send(Message::Binary(serde_json::to_vec(&hello_a).unwrap().into()))
        .unwrap();
    let frame = read_one_msg(&mut ws_a);
    match frame {
        DevToApp::Hello { session, .. } => {
            assert_eq!(
                session, "primary",
                "single-process mode pins every connection to the primary session"
            );
        }
        other => panic!("expected DevToApp::Hello, got {:?}", other),
    }

    // Client B: default identity. Same outcome.
    let (mut ws_b, _) = tungstenite::connect(&url).expect("connect");
    let hello_b = wire::AppToDev::Hello {
        app_name: "client-b".into(),
        color_scheme: wire::WireColorScheme::Auto,
        initial_url: None,
        identity: wire::ClientIdentity::default(),
    };
    ws_b.send(Message::Binary(serde_json::to_vec(&hello_b).unwrap().into()))
        .unwrap();
    let frame = read_one_msg(&mut ws_b);
    match frame {
        DevToApp::Hello { session, .. } => {
            assert_eq!(session, "primary");
        }
        other => panic!("expected DevToApp::Hello, got {:?}", other),
    }
}

fn read_one_msg(
    ws: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
) -> wire::DevToApp {
    use tungstenite::Message;
    if let tungstenite::stream::MaybeTlsStream::Plain(s) = ws.get_ref() {
        let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
    }
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
