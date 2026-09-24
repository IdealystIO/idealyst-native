//! End-to-end: a FULL-STACK `idealyst dev --web --local --events-file`
//! session — an app whose own server (built on the framework's
//! `server::router()`) serves the bundle and the API — reports the server
//! as a target of its own, builds it beside the bundle, reaches the page
//! same-origin, and keeps the server's output apart from the dev loop's.
//!
//! The fixture is materialized here, CrewForge's shape in miniature:
//!
//! ```text
//! <tmp>/dev_fullstack_e2e/
//!   app/     the web app (the project dir: server_manifest = "../server/Cargo.toml")
//!   server/  a standalone server crate on `server::router()`
//!   shared/  a crate both depend on
//! ```
//!
//! What it asserts, in order:
//!
//! 1. `session_started` declares the server (named after its bin, with its
//!    `server.log`), and the server's typed build — `build_started`,
//!    `cargo_progress`, `build_finished` — precedes `server_ready{full_stack}`;
//! 2. the server's first build and the bundle's overlapped in time;
//! 3. the page reaches the dev stream SAME-ORIGIN: the session reports
//!    `stream_route{same_origin}`, a relative `EventSource` on the page gets
//!    the snapshot through the app server's proxy, and the injected script's
//!    own stream and ack went through the app server (its request log);
//! 4. a save in the server's own sources rebuilds and restarts the server
//!    (`change_detected` → `build_finished{reloaded}` → `server_ready`) and
//!    builds nothing on the web side; the API answers with the new code;
//! 5. a save in `shared/` rebuilds BOTH, and the page reloads once, after
//!    the restarted server is ready;
//! 6. the server's request lines are in `server.log` and in the events as
//!    `output{source: server}`, and NOT in the panel's default log view;
//!    a panic in the server is an `error` from it naming `server.log`.
//!
//! `#[ignore]`d: it compiles the framework for wasm32 and for the host, and
//! needs Chrome (or `IDEALYST_BROWSER`) and `wasm-bindgen` on `PATH`:
//!
//! ```text
//! cargo test -p idealyst-cli --test dev_fullstack_e2e -- --ignored --nocapture
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crates/tools/cli has a workspace root 3 levels up")
        .to_path_buf()
}

// ── the project ─────────────────────────────────────────────────────────

const SHARED_V1: &str = r#"
pub fn greeting() -> &'static str {
    "greeting v1"
}
"#;

/// A shape change (a new item): the bundle cannot hot patch it.
const SHARED_V2: &str = r#"
pub fn greeting() -> &'static str {
    "greeting v2"
}

pub const REVISION: u32 = 2;
"#;

const APP_RS: &str = r#"
use runtime_core::{component, ui, Element};

#[component]
fn Root() -> Element {
    let line = format!("page says {}", shared::greeting());
    ui! {
        view {
            text { line }
        }
    }
}

pub fn app() -> Element {
    ui! { Root() }
}
"#;

const LIB_RS: &str = r#"
mod app;
pub use app::app;

pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}

pub fn scene_app() -> runtime_core::Element {
    app()
}
"#;

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
  <head><meta charset="utf-8" /><base href="/" /><title>dev fullstack e2e</title></head>
  <body>
    <div id="app"></div>
    <script type="module">
      import init from "/pkg/dev_fullstack_e2e.js";
      init();
    </script>
  </body>
</html>
"#;

/// The server: `server::router()` (which proxies `/__idealyst/*` under
/// `idealyst dev`), an API route, the staged bundle as the fallback, and
/// one log line per request — the "request log" the dev loop must keep
/// out of its own lines.
fn server_main(api: &str) -> String {
    format!(
        r#"
use axum::http::{{header, StatusCode, Uri}};
use axum::response::IntoResponse;

async fn log(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {{
    println!("request {{}} {{}}", req.method(), req.uri().path());
    next.run(req).await
}}

async fn bundle(uri: Uri) -> axum::response::Response {{
    let dist = std::path::PathBuf::from(std::env::var("WEB_DIST").expect("WEB_DIST"));
    let rel = uri.path().trim_start_matches('/');
    let path = if rel.is_empty() || rel.contains("..") || !dist.join(rel).is_file() {{
        dist.join("index.html")
    }} else {{
        dist.join(rel)
    }};
    let kind = match path.extension().and_then(|e| e.to_str()) {{
        Some("html") => "text/html",
        Some("js") => "text/javascript",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css",
        _ => "application/octet-stream",
    }};
    match std::fs::read(&path) {{
        Ok(bytes) => ([(header::CONTENT_TYPE, kind)], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }}
}}

#[tokio::main]
async fn main() {{
    let port = std::env::var("PORT").expect("PORT");
    let app = server::router()
        .route("/api/hello", axum::routing::get(|| async {{ {api} }}))
        .route("/api/panic", axum::routing::get(|| async {{
            if std::env::var_os("PORT").is_some() {{
                panic!("the e2e asked for a panic");
            }}
            "unreachable"
        }}))
        .fallback(bundle)
        .layer(axum::middleware::from_fn(log));
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{{port}}")).await.unwrap();
    println!("fixture-server listening on {{port}}");
    axum::serve(listener, app).await.unwrap();
}}
"#
    )
}

const API_V1: &str = r#"format!("api v1 / {}", shared::greeting())"#;
const API_V2: &str = r#"format!("api v2 / {}", shared::greeting())"#;

fn materialize(root: &Path, repo: &Path) {
    let dep = |p: &str| repo.join(p).display().to_string();
    for d in ["app/src", "server/src", "shared/src"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::write(
        root.join("shared/Cargo.toml"),
        "[package]\nname = \"shared\"\nversion = \"0.0.1\"\nedition = \"2021\"\npublish = false\n",
    )
    .unwrap();
    std::fs::write(root.join("shared/src/lib.rs"), SHARED_V1).unwrap();
    std::fs::write(
        root.join("app/Cargo.toml"),
        format!(
            r#"[package]
name = "dev-fullstack-e2e"
version = "0.0.1"
edition = "2021"
publish = false

[lib]
crate-type = ["rlib"]

[dependencies]
idealyst = {{ path = "{idealyst}" }}
runtime-core = {{ path = "{core}" }}
runtime-vocabulary = {{ path = "{vocab}" }}
runtime-scene = {{ path = "{scene}" }}
shared = {{ path = "../shared" }}

[package.metadata.idealyst.app]
name = "Dev Fullstack E2E"
bundle_id = "com.example.dev_fullstack_e2e"
version = "0.0.1"
targets = ["web"]
server_manifest = "../server/Cargo.toml"
server_bin = "fixture-server"

[workspace]
"#,
            idealyst = dep("crates/idealyst"),
            core = dep("crates/runtime/core"),
            vocab = dep("crates/runtime/vocabulary"),
            scene = dep("crates/runtime/scene"),
        ),
    )
    .unwrap();
    std::fs::write(root.join("app/src/lib.rs"), LIB_RS).unwrap();
    std::fs::write(root.join("app/src/app.rs"), APP_RS).unwrap();
    std::fs::write(root.join("app/src/main.rs"), "idealyst::entry!(dev_fullstack_e2e);\n").unwrap();
    std::fs::write(root.join("app/index.html"), INDEX_HTML).unwrap();
    std::fs::write(
        root.join("server/Cargo.toml"),
        format!(
            r#"[package]
name = "fixture-server"
version = "0.0.1"
edition = "2021"
publish = false

[[bin]]
name = "fixture-server"
path = "src/main.rs"

[dependencies]
server = {{ path = "{server}", features = ["server"] }}
shared = {{ path = "../shared" }}
axum = "0.7"
tokio = {{ version = "1", features = ["macros", "rt-multi-thread", "net"] }}

[workspace]
"#,
            server = dep("crates/api/server"),
        ),
    )
    .unwrap();
    std::fs::write(root.join("server/src/main.rs"), server_main(API_V1)).unwrap();
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

// ── the session ─────────────────────────────────────────────────────────

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Session {
    children: Vec<Child>,
    log: Arc<Mutex<String>>,
    events: PathBuf,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SIGINT first: the CLI's handler takes the server tree down.
        for child in &mut self.children {
            unsafe {
                libc::kill(child.id() as i32, libc::SIGINT);
            }
        }
        std::thread::sleep(Duration::from_millis(1500));
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Session {
    fn log(&self) -> String {
        self.log.lock().unwrap().clone()
    }

    fn events(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.events)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn wait_event(&self, after: u64, budget: Duration, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + budget;
        loop {
            if let Some(e) = self
                .events()
                .into_iter()
                .find(|e| e["seq"].as_u64().unwrap_or(0) > after && pred(e))
            {
                return e;
            }
            assert!(
                Instant::now() < deadline,
                "no {what} within {budget:?}. Events since {after}:\n{}\nLog tail:\n{}",
                summary(&self.events(), after),
                tail(&self.log()),
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn last_seq(&self) -> u64 {
        self.events().last().and_then(|e| e["seq"].as_u64()).unwrap_or(0)
    }
}

fn summary(events: &[Value], after: u64) -> String {
    events
        .iter()
        .filter(|e| e["seq"].as_u64().unwrap_or(0) > after)
        .filter(|e| !matches!(e["type"].as_str(), Some("output" | "cargo_progress" | "log")))
        .map(|e| e.to_string().chars().take(220).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn tail(log: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}

fn pump(reader: impl Read + Send + 'static, log: Arc<Mutex<String>>) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            eprintln!("{line}");
            let mut log = log.lock().unwrap();
            log.push_str(&line);
            log.push('\n');
        }
    });
}

fn browser() -> Option<String> {
    if let Ok(p) = std::env::var("IDEALYST_BROWSER") {
        return Path::new(&p).exists().then_some(p);
    }
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ]
    .iter()
    .find(|p| Path::new(p).exists())
    .map(|p| p.to_string())
}

/// A minimal GET, returning the body; reads exactly `Content-Length`
/// bytes, or to the end when there is none.
fn http_get(port: u16, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").ok()?;
    let mut reader = BufReader::new(stream);
    let mut length = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                length = v.trim().parse::<usize>().ok();
            }
        }
    }
    let body = match length {
        Some(n) => {
            let mut body = vec![0u8; n];
            reader.read_exact(&mut body).ok()?;
            body
        }
        None => {
            let mut body = Vec::new();
            reader.read_to_end(&mut body).ok()?;
            body
        }
    };
    String::from_utf8(body).ok()
}

/// A DevTools-protocol connection to the app's page.
struct Page {
    ws: tungstenite::WebSocket<TcpStream>,
    next_id: u64,
}

impl Page {
    fn attach(debug_port: u16) -> Page {
        let deadline = Instant::now() + Duration::from_secs(30);
        let ws_url = loop {
            let page = http_get(debug_port, "/json/list")
                .and_then(|body| serde_json::from_str::<Value>(&body).ok())
                .and_then(|list| {
                    list.as_array()?.iter().find_map(|t| {
                        let is_app = t["type"] == "page"
                            && t["url"].as_str().is_some_and(|u| u.starts_with("http"));
                        is_app.then(|| t["webSocketDebuggerUrl"].as_str().map(str::to_string))?
                    })
                });
            if let Some(url) = page {
                break url;
            }
            assert!(Instant::now() < deadline, "Chrome never exposed the app's page target");
            std::thread::sleep(Duration::from_millis(200));
        };
        let stream = TcpStream::connect(("127.0.0.1", debug_port)).unwrap();
        let (ws, _) = tungstenite::client(ws_url.as_str(), stream).expect("CDP handshake");
        Page { ws, next_id: 1 }
    }

    fn eval(&mut self, expr: &str) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "id": id,
            "method": "Runtime.evaluate",
            "params": { "expression": expr, "returnByValue": true, "awaitPromise": true },
        });
        self.ws.send(tungstenite::Message::text(msg.to_string())).unwrap();
        loop {
            let reply = self.ws.read().expect("CDP read");
            let tungstenite::Message::Text(text) = reply else { continue };
            let v: Value = serde_json::from_str(&text).unwrap();
            if v["id"].as_u64() == Some(id) {
                return v["result"]["result"]["value"].clone();
            }
        }
    }

    fn wait(&mut self, expr: &str, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            if self.eval(expr) == json!(true) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        false
    }

    fn text(&mut self) -> String {
        self.eval("document.body ? document.body.innerText : ''").as_str().unwrap_or_default().to_string()
    }
}

/// `(start at_ms, finish at_ms)` of `target`'s first build.
fn first_build(events: &[Value], target: &str) -> (u64, u64) {
    let at = |ty: &str| {
        events
            .iter()
            .find(|e| e["type"] == ty && e["target"] == target)
            .and_then(|e| e["at_ms"].as_u64())
            .unwrap_or_else(|| panic!("no {ty} for {target}: {}", summary(events, 0)))
    };
    (at("build_started"), at("build_finished"))
}

fn seq(e: &Value) -> u64 {
    e["seq"].as_u64().unwrap()
}

#[test]
#[ignore = "compiles the framework for wasm32 and the host and drives headless Chrome; run with --ignored"]
fn a_full_stack_session_builds_and_reports_its_server_as_a_target() {
    let repo = repo_root();
    // A stable directory: the web target dir is keyed by the project path
    // while the hot tier is armed, so a fresh path is a cold build.
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dev_fullstack_e2e");
    materialize(&root, &repo);
    let project = root.join("app");
    let scratch = tempfile::tempdir().unwrap();
    let chrome =
        browser().expect("this test needs Chrome/Chromium, or IDEALYST_BROWSER pointing at one");

    let log = Arc::new(Mutex::new(String::new()));
    let events_path = scratch.path().join("events.jsonl");
    let mut session = Session { children: Vec::new(), log: log.clone(), events: events_path.clone() };
    let port = free_port();
    let mut dev = Command::new(env!("CARGO_BIN_EXE_idealyst"))
        .current_dir(&project)
        .env("IDEALYST_TEST_DRIVER", "1")
        .args(["dev", "--web", "--local", "--no-robot", "--no-headless-client"])
        .args(["--port", &port.to_string()])
        .arg("--events-file")
        .arg(&events_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn idealyst dev");
    pump(dev.stdout.take().unwrap(), log.clone());
    pump(dev.stderr.take().unwrap(), log.clone());
    session.children.push(dev);

    // ── 1. the server is a target, typed from its first build ─────────
    let ready = session.wait_event(0, Duration::from_secs(1800), "server_ready(full_stack)", |e| {
        e["type"] == "server_ready" && e["kind"] == "full_stack"
    });
    let events = session.events();
    let started = &events[0];
    assert_eq!(started["type"], "session_started", "{}", summary(&events, 0));
    assert_eq!(started["server"]["target"], "server");
    assert_eq!(started["server"]["name"], "fixture-server");
    let server_log = PathBuf::from(started["server"]["log_file"].as_str().expect("the server's log file"));
    assert!(server_log.ends_with("target/idealyst/dev-fullstack-e2e/server.log"), "{}", server_log.display());
    let server_typed: Vec<&Value> = events
        .iter()
        .filter(|e| e["target"] == "server" && seq(e) < seq(&ready))
        .collect();
    let kinds: Vec<&str> = server_typed.iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(kinds.contains(&"build_started"), "{kinds:?}");
    assert!(kinds.contains(&"stage_started"), "{kinds:?}");
    assert!(kinds.contains(&"cargo_progress"), "{kinds:?}");
    assert!(
        server_typed.iter().any(|e| e["type"] == "build_finished" && e["outcome"] == "ready"),
        "the server's build finished ready before server_ready: {kinds:?}"
    );

    // ── 2. the two first builds overlapped ───────────────────────────
    let web_build = session.wait_event(0, Duration::from_secs(1800), "the web build", |e| {
        e["type"] == "build_finished" && e["target"] == "web"
    });
    assert_eq!(web_build["outcome"], "ready", "{web_build}");
    let events = session.events();
    let (ws, wf) = first_build(&events, "web");
    let (ss, sf) = first_build(&events, "server");
    eprintln!("[e2e] web build {ws}..{wf} ms, server build {ss}..{sf} ms");
    assert!(ss < wf && ws < sf, "the builds did not overlap: web {ws}..{wf}, server {ss}..{sf}");

    // ── 3. the page reaches the stream same-origin ────────────────────
    let route = session.wait_event(0, Duration::from_secs(30), "stream_route", |e| e["type"] == "stream_route");
    assert_eq!(route["route"], "same_origin", "{route}");
    let debug_port = free_port();
    session.children.push(
        Command::new(&chrome)
            .args([
                "--headless=new",
                "--disable-gpu",
                "--no-sandbox",
                "--disable-dev-shm-usage",
                "--no-first-run",
                "--no-default-browser-check",
            ])
            .arg(format!("--remote-debugging-port={debug_port}"))
            .arg(format!("--user-data-dir={}", scratch.path().join("chrome-profile").display()))
            .arg(format!("http://127.0.0.1:{port}/"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn Chrome"),
    );
    let mut page = Page::attach(debug_port);
    assert!(
        page.wait("document.body.innerText.includes('page says greeting v1')", Duration::from_secs(120)),
        "the app never rendered: {:?}",
        page.text()
    );
    session.wait_event(0, Duration::from_secs(30), "the page acking its connection", |e| {
        e["type"] == "page_ack" && e["ack"]["kind"] == "connected"
    });
    // A relative EventSource — the app server's proxy — gets the snapshot.
    page.eval(
        "(() => { window.__e2e_states = []; \
           const es = new EventSource('/__idealyst/reload'); \
           es.addEventListener('dev-state', e => window.__e2e_states.push(JSON.parse(e.data))); \
           return new Promise(r => es.onopen = () => r(true)); })()",
    );
    assert!(
        page.wait(
            "(window.__e2e_states || []).some(s => s.type === 'session_started' && s.server && s.server.name === 'fixture-server')",
            Duration::from_secs(15)
        ),
        "no snapshot through the same-origin proxy: {}",
        page.eval("JSON.stringify(window.__e2e_states || [])")
    );
    // The injected script's own stream and its ack went through the app
    // server too (its request log names both).
    let proxied = std::fs::read_to_string(&server_log).unwrap_or_default();
    assert!(proxied.contains("request GET /__idealyst/reload"), "{proxied}");
    assert!(proxied.contains("request POST /__idealyst/ack"), "{proxied}");

    // ── 4. a server-only save ────────────────────────────────────────
    assert_eq!(http_get(port, "/api/hello").as_deref(), Some("api v1 / greeting v1"));
    let before = session.last_seq();
    write(&root.join("server/src/main.rs"), &server_main(API_V2));
    let restarted = session.wait_event(before, Duration::from_secs(600), "the restarted server", |e| {
        e["type"] == "server_ready" && e["kind"] == "full_stack"
    });
    let events = session.events();
    let window: Vec<&Value> =
        events.iter().filter(|e| seq(e) > before && seq(e) <= seq(&restarted)).collect();
    let server_kinds: Vec<String> = window
        .iter()
        .filter(|e| e["target"] == "server" && e["type"] != "output" && e["type"] != "cargo_progress")
        .map(|e| match e["type"].as_str().unwrap() {
            "build_finished" => format!("build_finished:{}", e["outcome"].as_str().unwrap()),
            t => t.to_string(),
        })
        .collect();
    let expected = ["change_detected", "build_started", "build_finished:reloaded"];
    let mut it = server_kinds.iter();
    assert!(expected.iter().all(|k| it.any(|s| s == k)), "{server_kinds:?}");
    assert_eq!(http_get(port, "/api/hello").as_deref(), Some("api v2 / greeting v1"));
    // The web side built nothing for it.
    std::thread::sleep(Duration::from_secs(3));
    let web_after: Vec<Value> = session
        .events()
        .into_iter()
        .filter(|e| {
            seq(e) > before
                && e["target"] == "web"
                && (e["type"] == "build_started" || e["type"] == "change_detected")
        })
        .collect();
    assert!(web_after.is_empty(), "a server-only save touched the web bundle: {web_after:?}");

    // ── 5. a save both sides depend on ────────────────────────────────
    let before = session.last_seq();
    write(&root.join("shared/src/lib.rs"), SHARED_V2);
    let reloading = session.wait_event(before, Duration::from_secs(900), "the page reloading", |e| {
        e["type"] == "page_ack" && e["ack"]["kind"] == "reloading"
    });
    let events = session.events();
    let find = |pred: &dyn Fn(&Value) -> bool| events.iter().find(|e| seq(e) > before && pred(e)).cloned();
    let web_done = find(&|e| e["type"] == "build_finished" && e["target"] == "web").expect("the web rebuild");
    let server_done =
        find(&|e| e["type"] == "build_finished" && e["target"] == "server").expect("the server rebuild");
    let server_back = find(&|e| e["type"] == "server_ready" && e["kind"] == "full_stack").expect("the restart");
    assert_eq!(web_done["outcome"], "reloaded", "{web_done}");
    assert_eq!(server_done["outcome"], "reloaded", "{server_done}");
    assert!(
        seq(&server_back) < seq(&reloading),
        "the page reloaded before the restarted server was ready:\n{}",
        summary(&events, before)
    );
    assert!(
        page.wait("document.body.innerText.includes('page says greeting v2')", Duration::from_secs(120)),
        "{:?}",
        page.text()
    );
    std::thread::sleep(Duration::from_secs(3));
    let reloads = session
        .events()
        .iter()
        .filter(|e| seq(e) > before && e["type"] == "page_ack" && e["ack"]["kind"] == "reloading")
        .count();
    assert_eq!(reloads, 1, "the page reloads once for a save both sides depend on");
    assert_eq!(http_get(port, "/api/hello").as_deref(), Some("api v2 / greeting v2"));

    // ── 6. the server's output is its own ─────────────────────────────
    let server_output = std::fs::read_to_string(&server_log).unwrap();
    assert!(server_output.contains("request GET /api/hello"), "{server_output}");
    assert!(server_output.contains("fixture-server listening on"), "{server_output}");
    let events = session.events();
    assert!(
        events.iter().any(|e| e["type"] == "output"
            && e["source"] == "server"
            && e["target"] == "server"
            && e["line"].as_str().is_some_and(|l| l.contains("request GET /api/hello"))),
        "the request lines are tagged server output"
    );
    // The panel's default log view (the dev loop's lines) leaves them out;
    // its server view has them.
    let mut model = dev_tui::model::Model::new(&["web".to_string()]);
    for e in &events {
        let envelope: dev_events::Envelope = serde_json::from_value(e.clone()).unwrap();
        model.apply(&envelope);
    }
    let pane = |view| {
        dev_tui::view::screen(&model, dev_tui::view::Toggles { log: view, expanded: false }, 0, 0, 200, 400)
            .log
            .into_iter()
            .map(|l| l.text)
            .collect::<Vec<_>>()
    };
    let dev_view = pane(dev_tui::view::LogView::Dev);
    assert!(!dev_view.iter().any(|l| l.contains("request GET")), "server chatter in the dev view: {dev_view:#?}");
    assert!(dev_view.iter().any(|l| l.contains("Compiling")), "{dev_view:#?}");
    let server_view = pane(dev_tui::view::LogView::Server);
    assert!(server_view.iter().any(|l| l.contains("[server] request GET /api/hello")), "{server_view:#?}");
    // A plain terminal still shows them, tagged.
    assert!(session.log().contains("[server] request GET /api/hello"), "{}", tail(&session.log()));

    // A panic in the server (a request handler's) is an error on its row,
    // naming the log that has the rest of its output.
    let before = session.last_seq();
    let _ = http_get(port, "/api/panic");
    let panic = session.wait_event(before, Duration::from_secs(30), "the server's panic as an error", |e| {
        e["type"] == "error" && e["source"] == "server"
    });
    let message = panic["message"].as_str().unwrap();
    assert!(message.contains("panicked at"), "{panic}");
    assert!(message.contains(&server_log.display().to_string()), "{panic}");

    // Put the sources back so the next run starts from v1.
    drop(page);
    drop(session);
    materialize(&root, &repo);
}
