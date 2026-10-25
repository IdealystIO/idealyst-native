//! The Robot bridge's `screenshot` verb answers on GTK, end to end.
//!
//! ## The bug this guards
//!
//! GTK delivers a capture on a later main-loop turn: `screenshot::capture`
//! forces a draw and reads the widget paintable once GTK has painted it.
//! The bridge verb read its result slot straight after calling the capture,
//! so on GTK it always answered "capture_screenshot did not complete
//! synchronously" — the verb was registered but could never succeed, and the
//! MCP robot tools / `idealyst test --parity` got no picture of a Linux app.
//!
//! This mounts a real window, sends `{"cmd":"screenshot"}` over a real TCP
//! bridge connection (what an MCP client does), pumps GTK and the bridge,
//! and requires a PNG back: the end-to-end check that the verb works on GTK.
//! It does not by itself tell the bug apart: in this harness GTK has usually
//! painted already, so the capture takes its inline fast path, and the
//! deferred path can't be forced deterministically here. The deferred reply
//! is pinned by `runtime-shared`'s
//! `regression_a_capture_that_completes_later_still_answers`, which fails
//! against the old verb.
//!
//! One `#[test]`: GTK must be driven from the thread that ran `gtk::init`.
//! Needs the `robot` feature (`cargo test -p backend-linux --features robot`).

#![cfg(all(target_os = "linux", feature = "robot"))]

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use backend_linux::{gtk4, newcore, LinuxBackend};
use gtk4::prelude::*;
use runtime_shared::{Color, StyleRules, StyleSheet};
use runtime_vocabulary::builders::{text, view};

#[test]
fn screenshot_verb_answers_on_gtk() {
    if gtk4::init().is_err() {
        eprintln!("SKIP: no display / GTK init failed");
        return;
    }
    let window = gtk4::Window::new();
    window.set_default_size(320, 240);
    let backend = Rc::new(RefCell::new(LinuxBackend::new(window.clone())));
    backend.borrow_mut().set_self_ref(Rc::downgrade(&backend));
    let app = newcore::start(backend.clone(), |_r| {}, || {
        view()
            .style(Rc::new(StyleSheet::new(|_| StyleRules {
                background: Some(Color("#ff0000".into()).into()),
                ..Default::default()
            })))
            .child(text().content("shot").build())
            .build()
    });
    window.present();
    let ctx = gtk4::glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !window.is_mapped() && Instant::now() < deadline {
        ctx.iteration(false);
    }
    if !window.is_mapped() {
        eprintln!("SKIP: window never mapped in this environment");
        return;
    }
    backend.borrow_mut().run_layout(320.0, 240.0);
    for _ in 0..200 {
        ctx.iteration(false);
    }

    let (handle, port) = runtime_shared::robot::bridge::start_on_port(0).expect("bind bridge");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.write_all(b"{\"id\":1,\"cmd\":\"screenshot\",\"args\":{}}\n").expect("send");
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).expect("read");
        let _ = tx.send(line);
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut line = None;
    while Instant::now() < deadline {
        handle.poll();
        ctx.iteration(false);
        if let Ok(l) = rx.try_recv() {
            line = Some(l);
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    app.stop();

    let line = line.expect("the bridge answered the screenshot request");
    let v: serde_json::Value = serde_json::from_str(line.trim()).expect("json response");
    assert!(v.get("err").is_none(), "the screenshot verb failed on GTK: {line}");
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode(v["ok"]["png_base64"].as_str().expect("png_base64"))
        .expect("base64");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "a PNG came back");
    assert!(v["ok"]["width"].as_u64().unwrap_or(0) > 0);
}
