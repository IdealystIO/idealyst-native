//! Stand up a robot-relay + serve a robot-enabled web bundle, for manually /
//! interactively verifying the browser→relay handshake (e.g. driving the page
//! with Playwright while querying the relay's TCP bridge).
//!
//! Usage:  relay-serve <path/to/dist/web>
//!
//! Stages the bundle into a temp dir, injects `window.IDEALYST_ROBOT_RELAY_URL`
//! pointing at the relay, serves it via `python3 -m http.server`, prints the
//! ports, and parks. Load the printed URL in a browser; the app dials the
//! relay on boot. Then drive verbs against `TCP_PORT`, e.g.:
//!   printf '{"id":1,"cmd":"get_snapshot","args":{}}\n' | nc 127.0.0.1 <TCP_PORT>

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

fn main() {
    let dist = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: relay-serve <path/to/dist/web>"),
    );
    assert!(
        dist.join("index.html").is_file(),
        "no index.html under {}",
        dist.display()
    );

    let relay = robot_relay::start(robot_relay::RelayConfig {
        ws_port: 0,
        tcp_port: 0,
        register: false,
        identity: None,
        screenshot_dir: None,
    })
    .expect("relay starts");
    let ws_url = format!("ws://127.0.0.1:{}", relay.ws_addr.port());

    // Stage + inject.
    let tmp = std::env::temp_dir().join("relay_serve_stage");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let staged = tmp.join("web");
    assert!(
        Command::new("cp")
            .arg("-R")
            .arg(&dist)
            .arg(&staged)
            .status()
            .unwrap()
            .success(),
        "copy dist failed"
    );
    let index = staged.join("index.html");
    let html = std::fs::read_to_string(&index).unwrap();
    let snippet = format!("<script>window.IDEALYST_ROBOT_RELAY_URL=\"{ws_url}\";</script>\n");
    let out = match html.find("</head>") {
        Some(i) => format!("{}{}{}", &html[..i], snippet, &html[i..]),
        None => format!("{snippet}{html}"),
    };
    std::fs::write(&index, out).unwrap();

    // Serve.
    let serve_port = free_port();
    let _server = Command::new("python3")
        .args(["-m", "http.server", &serve_port.to_string(), "--bind", "127.0.0.1"])
        .arg("--directory")
        .arg(&staged)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("python3 http.server");

    println!("SERVE_PORT={serve_port}");
    println!("TCP_PORT={}", relay.tcp_addr.port());
    println!("WS_PORT={}", relay.ws_addr.port());
    println!("URL=http://127.0.0.1:{serve_port}/");
    std::io::stdout().flush().unwrap();

    // Park; relay handle + server child stay alive in scope.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
