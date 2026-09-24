//! `/__idealyst/events`: the session's event stream over HTTP.
//!
//! What a tool with nothing but the port gets: first a snapshot of the
//! session as it is (so it can render without history), then every event
//! live, each a versioned JSON object on one SSE `data:` line — the same
//! objects `--events-file` writes. Checked on the stand-alone server
//! every session runs and on the dev server beside the page.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use dev_events::broadcast::Broadcast;
use dev_events::{BuildCause, DevEvent, Envelope, Reporter};
use dev_http::{serve_events, serve_signal_only, EVENTS_URL};
use dev_reload::ReloadSignal;

fn pick_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    port
}

fn wait_for(port: u16) {
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("nothing bound {port}");
}

/// Open the stream and read `n` events off it.
fn subscribe(port: u16) -> impl FnMut() -> Envelope {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(stream, "GET {EVENTS_URL} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut reader = BufReader::new(stream);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        head.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.contains("text/event-stream"), "{head}");
    assert!(head.contains("Access-Control-Allow-Origin: *"), "{head}");
    move || loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("an event before the timeout");
        if let Some(json) = line.strip_prefix("data: ") {
            return serde_json::from_str(json.trim_end()).expect(json);
        }
    }
}

fn session() -> (Reporter, Arc<Broadcast>) {
    let r = Reporter::new();
    let b = Broadcast::new();
    r.add_sink(b.clone());
    (r, b)
}

#[test]
fn a_late_subscriber_gets_the_snapshot_then_live_events() {
    let (r, events) = session();
    r.emit(DevEvent::BuildStarted { target: "web".into(), cause: BuildCause::Initial });
    r.log("dev", "history — not in the snapshot");
    let port = pick_port();
    thread::spawn(move || {
        let _ = serve_events("127.0.0.1", port, events);
    });
    wait_for(port);

    let mut next = subscribe(port);
    let first = next();
    assert_eq!(first.seq, 1);
    assert!(matches!(first.event, DevEvent::BuildStarted { .. }));
    assert_eq!(first.v, dev_events::SCHEMA_VERSION);

    // Live, after the snapshot. Give the subscription a moment to attach
    // (the snapshot arrived, so it has).
    r.emit(DevEvent::OverlayPushed { target: "web".into(), sites: 2, ms: 9 });
    let live = next();
    assert_eq!(live.seq, 3, "the log line was seq 2 and is history, not state");
    assert!(matches!(live.event, DevEvent::OverlayPushed { sites: 2, .. }));
}

#[test]
fn the_reload_stream_server_serves_it_too() {
    let (r, events) = session();
    let signal = ReloadSignal::new();
    signal.serve_events(events);
    let port = pick_port();
    let s = signal.clone();
    thread::spawn(move || {
        let _ = serve_signal_only("127.0.0.1", port, s);
    });
    wait_for(port);
    let mut next = subscribe(port);
    // Nothing yet: the snapshot is empty, so the first event is live.
    thread::sleep(Duration::from_millis(100));
    r.emit(DevEvent::BuildStarted { target: "web".into(), cause: BuildCause::Forced });
    assert!(matches!(next().event, DevEvent::BuildStarted { cause: BuildCause::Forced, .. }));
}
