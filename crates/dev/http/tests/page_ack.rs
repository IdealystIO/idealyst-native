//! A page's report of what it did with a pushed event reaches the dev
//! session.
//!
//! Before this endpoint, "overlay patch: 1 applied" and "hot patch: 3
//! function(s) redirected" existed only in the browser console: the
//! terminal could say a patch was SENT, never that it landed. Both
//! servers — the static one and the stand-alone stream beside a
//! full-stack app — take a `POST` at `ACK_URL` and hand the body to the
//! signal, which reports it as a typed event.
//!
//! Raw `TcpStream`, like the other tests here: no HTTP client dependency.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use dev_events::{DevEvent, PageAck, Queue, Reporter};
use dev_http::{reload_script_tag, serve_signal_only, serve_static, ReloadContext, ACK_URL};
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
    panic!("the server never bound {port}");
}

/// POST `body` the way `navigator.sendBeacon` does, and return the
/// response head.
fn beacon(port: u16, body: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "POST {ACK_URL} HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://127.0.0.1:3100\r\n\
         Content-Type: text/plain;charset=UTF-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out);
    out
}

fn listening(signal: &ReloadSignal) -> Queue {
    let r = Reporter::new();
    let q = Queue::new();
    r.add_sink(Arc::new(q.clone()));
    signal.report_acks_to(r);
    q
}

fn wait_for_events(q: &Queue, n: usize) -> Vec<DevEvent> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut got = Vec::new();
    while got.len() < n && Instant::now() < deadline {
        got.extend(q.drain().into_iter().map(|e| e.event));
        thread::sleep(Duration::from_millis(10));
    }
    got
}

#[test]
fn the_stand_alone_stream_takes_a_cross_origin_ack() {
    let port = pick_port();
    let signal = ReloadSignal::new();
    let q = listening(&signal);
    let for_server = signal.clone();
    thread::spawn(move || {
        let _ = serve_signal_only("127.0.0.1", port, for_server);
    });
    wait_for(port);

    let head = beacon(port, r#"{"kind":"overlay","applied":1,"refused":0}"#);
    assert!(head.starts_with("HTTP/1.1 204"), "{head}");
    // The page is on the app server's origin; without this the browser
    // would still send the beacon, but a `fetch` fallback would fail.
    assert!(head.contains("Access-Control-Allow-Origin: *"), "{head}");
    assert_eq!(
        wait_for_events(&q, 1),
        vec![DevEvent::PageAck {
            target: "web".into(),
            ack: PageAck::Overlay { applied: Some(1), refused: Some(0) },
        }]
    );
}

#[test]
fn the_static_server_takes_an_ack_and_ignores_what_it_does_not_understand() {
    let port = pick_port();
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("index.html"), "<html></html>").unwrap();
    let signal = ReloadSignal::new();
    let q = listening(&signal);
    let ctx = ReloadContext { signal: signal.clone() };
    let dir = root.path().to_path_buf();
    thread::spawn(move || {
        let _ = serve_static("127.0.0.1", port, &dir, Some(ctx), None, None, None, None, None, false);
    });
    wait_for(port);

    assert!(beacon(port, r#"{"kind":"nonsense"}"#).starts_with("HTTP/1.1 204"));
    assert!(beacon(port, r#"{"kind":"hot_patch","redirected":3,"carried":12}"#)
        .starts_with("HTTP/1.1 204"));
    assert_eq!(
        wait_for_events(&q, 1),
        vec![DevEvent::PageAck {
            target: "web".into(),
            ack: PageAck::HotPatch { redirected: Some(3), carried: Some(12) },
        }],
        "the unknown ack is dropped, the known one reported"
    );
}

/// The ack endpoint is found from the stream's URL, so the full-stack
/// page (stream on another port) posts to the stream's origin, and the
/// static page posts same-origin.
#[test]
fn the_page_script_acks_beside_the_stream_it_listens_to() {
    let absolute = reload_script_tag("http://127.0.0.1:5123/__idealyst/reload");
    assert!(absolute.contains(r#"var ACK = "http://127.0.0.1:5123/__idealyst/ack";"#), "{absolute}");
    let relative = reload_script_tag("/__idealyst/reload");
    assert!(relative.contains(r#"var ACK = "/__idealyst/ack";"#), "{relative}");
    for script in [&absolute, &relative] {
        for kind in ["connected", "reloading", "overlay", "failed"] {
            assert!(script.contains(&format!("kind: \"{kind}\"")), "no {kind} ack in {script}");
        }
        assert!(
            script.contains("window.__idealyst_dev_ack = ack"),
            "the bundle acks a hot patch through this"
        );
    }
}
