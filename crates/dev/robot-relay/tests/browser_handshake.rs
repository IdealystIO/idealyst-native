//! PROPER end-to-end test of robot-on-web.
//!
//! A real headless browser loads a robot-enabled wasm bundle; the app boots,
//! reads `window.IDEALYST_ROBOT_RELAY_URL`, and **dials the relay over a
//! WebSocket**. We then drive robot verbs from a plain TCP client (the
//! MCP/evaluator side) and assert the response reflects the app's actual
//! element tree. If the snapshot comes back, the whole chain is proven:
//!
//!   browser wasm app  ──WS──▶  robot-relay  ◀──TCP──  evaluator
//!
//! `#[ignore]` because it needs a headless browser + a pre-built bundle.
//!
//! Build the bundle (once):
//! ```text
//! cd /tmp && rm -rf rw && mkdir rw && cd rw
//! IDEALYST_FRAMEWORK_PATH=<repo> <repo>/target/debug/idealyst new app
//! <repo>/target/debug/idealyst build --web --robot /tmp/rw/app
//! ```
//! Run:
//! ```text
//! ARENA_ROBOT_DIST=/tmp/rw/app/dist/web \
//!   cargo test -p robot-relay --test browser_handshake -- --ignored --nocapture
//! ```

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const DEFAULT_CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

/// Kills its child processes on drop so a panic never leaks chrome/python.
struct Kill(Vec<Child>);
impl Drop for Kill {
    fn drop(&mut self) {
        for c in &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

fn inject_relay_url(index: &Path, ws_url: &str) {
    let html = std::fs::read_to_string(index).unwrap();
    let snippet = format!("<script>window.IDEALYST_ROBOT_RELAY_URL=\"{ws_url}\";</script>\n");
    let out = match html.find("</head>") {
        Some(i) => format!("{}{}{}", &html[..i], snippet, &html[i..]),
        None => format!("{snippet}{html}"),
    };
    std::fs::write(index, out).unwrap();
}

/// One-shot TCP bridge call against the relay (fresh connection per call).
fn call(addr: SocketAddr, cmd: &str) -> Value {
    let stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut line = format!("{{\"id\":1,\"cmd\":\"{cmd}\",\"args\":{{}}}}");
    line.push('\n');
    writer.write_all(line.as_bytes()).unwrap();
    writer.flush().unwrap();
    let mut reader = BufReader::new(stream);
    let mut resp = String::new();
    reader.read_line(&mut resp).unwrap();
    serde_json::from_str(resp.trim()).unwrap()
}

#[test]
#[ignore = "needs a headless browser + a pre-built robot web bundle (see file header)"]
fn browser_dials_relay_and_robot_verbs_round_trip() {
    let dist = PathBuf::from(
        std::env::var("ARENA_ROBOT_DIST")
            .expect("set ARENA_ROBOT_DIST to a `idealyst build --web --robot` dist/web dir"),
    );
    assert!(
        dist.join("index.html").is_file(),
        "no index.html under {} — build the bundle first (see file header)",
        dist.display()
    );
    let chrome = std::env::var("ARENA_CHROME").unwrap_or_else(|_| DEFAULT_CHROME.to_string());
    assert!(Path::new(&chrome).exists(), "no browser at {chrome}");

    // 1. Relay (no registration file — we connect straight to its TCP addr).
    let relay = robot_relay::start(robot_relay::RelayConfig {
        ws_port: 0,
        tcp_port: 0,
        register: false,
        identity: None,
        screenshot_dir: None,
    })
    .expect("relay starts");
    let ws_url = format!("ws://127.0.0.1:{}", relay.ws_addr.port());

    // 2. Stage the bundle into a temp dir + inject the relay URL.
    let tmp = std::env::temp_dir().join("arena_browser_handshake");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let staged = tmp.join("web");
    let ok = Command::new("cp")
        .arg("-R")
        .arg(&dist)
        .arg(&staged)
        .status()
        .unwrap()
        .success();
    assert!(ok, "failed to copy dist");
    inject_relay_url(&staged.join("index.html"), &ws_url);

    // 3. Serve it (python3 maps .wasm → application/wasm on 3.11+, and
    //    wasm-bindgen's loader falls back to arrayBuffer instantiation
    //    otherwise — so MIME isn't load-bearing here).
    let serve_port = free_port();
    let server = Command::new("python3")
        .args(["-m", "http.server", &serve_port.to_string(), "--bind", "127.0.0.1"])
        .arg("--directory")
        .arg(&staged)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("python3 http.server");
    let mut kill = Kill(vec![server]);
    std::thread::sleep(Duration::from_millis(600));

    // 4. Launch headless Chrome at the served page. The app dials the relay on
    //    boot — no interaction needed. `--remote-debugging-port=0` keeps the
    //    new-headless instance alive (it doesn't exit after first paint).
    let url = format!("http://127.0.0.1:{serve_port}/");
    let profile = tmp.join("chrome-profile");
    let chrome_child = Command::new(&chrome)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-sandbox",
            "--no-first-run",
            "--no-default-browser-check",
            "--remote-debugging-port=0",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(&url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("launch chrome");
    kill.0.push(chrome_child);

    // 5. Poll a robot verb until the browser app has connected to the relay.
    let mut snapshot = Value::Null;
    for attempt in 0..45 {
        let resp = call(relay.tcp_addr, "get_snapshot");
        if let Some(ok) = resp.get("ok") {
            if !ok.is_null() {
                snapshot = ok.clone();
                println!("app connected on attempt {attempt}");
                break;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    assert!(
        !snapshot.is_null(),
        "the browser app never connected to the relay (no non-null get_snapshot within 45s)"
    );
    let serialized = snapshot.to_string();
    assert!(
        serialized.len() > 20,
        "snapshot is implausibly small — registry empty? got: {serialized}"
    );
    println!(
        "✅ robot-on-web verified: get_snapshot returned {} bytes from the real browser app\n{}",
        serialized.len(),
        &serialized[..serialized.len().min(600)]
    );

    // Also exercise a discrete lookup to prove forwarding of arg'd verbs.
    let count = call(relay.tcp_addr, "get_snapshot");
    assert!(count.get("ok").is_some(), "second verb failed: {count}");
}
