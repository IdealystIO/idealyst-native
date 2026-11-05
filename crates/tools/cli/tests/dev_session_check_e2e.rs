//! End-to-end: `idealyst check --session` builds through a running full-stack
//! session without applying anything, and `[package.metadata.idealyst.dev]`
//! chooses a session's build. Each test drives the real binary, cargo, server
//! and (where the page matters) headless Chrome:
//!
//! - `a_check_applies_nothing_and_the_next_rebuild_reuses_it`: a warm check
//!   compiles nothing; after an edit it compiles, the server keeps its pid and
//!   the page keeps its state; a compile error is a failing status with its
//!   location, and the app is untouched; `--no-watch` saves stay queued; the
//!   real rebuild after it compiles nothing, links, restarts and reloads.
//! - `a_hot_patch_after_a_check_reaches_the_running_program`: the check relinks
//!   the server's binary and rewrites the rlibs, and a further body edit still
//!   patches the running server and page, which the base must still describe.
//! - `regression_a_save_with_no_gap_before_its_request_is_applied`: under
//!   `--no-watch`, a request sent right after a save, before the watcher has
//!   reported it, applies that save on the web and on the server, and leaves
//!   nothing queued.
//! - `a_test_harness_check_compiles_only_the_harness`: `--tests -p <crate>`
//!   reuses the server's units, though the crate alone resolves a dependency
//!   with fewer features.
//! - `the_project_configuration_chooses_the_build`: a configured session uses
//!   the flagged command's directory, a differing flag warns before building,
//!   a behaviour flag does not, and a one-off override leaves the configured
//!   directory warm.
//!
//! `#[ignore]`d: they compile the framework for wasm32 and the host, and need
//! Chrome (or `IDEALYST_BROWSER`). A body edit in a running session reaches
//! every crate it watches, so on a tree others are editing, point the fixture
//! at a snapshot of the framework:
//!
//! ```text
//! git archive HEAD | tar -x -C /tmp/frozen
//! IDEALYST_E2E_FRAMEWORK_ROOT=/tmp/frozen \
//!   cargo test -p idealyst-cli --test dev_session_check_e2e -- --ignored --nocapture --test-threads=1
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const COLD: Duration = Duration::from_secs(1800);

fn framework_root() -> PathBuf {
    match std::env::var_os("IDEALYST_E2E_FRAMEWORK_ROOT") {
        Some(p) => PathBuf::from(p),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(3).expect("workspace root").to_path_buf(),
    }
}

fn shared(greeting: &str) -> String {
    format!(
        r#"
pub fn greeting() -> String {{
    let words = ["greeting", "{greeting}"];
    words.join(" ")
}}

pub fn permits() -> tokio::sync::Semaphore {{
    tokio::sync::Semaphore::new(1)
}}

#[cfg(test)]
mod tests {{
    #[test]
    fn it_greets() {{
        assert!(super::greeting().starts_with("greeting"));
    }}
}}
"#
    )
}

const SHARED_BROKEN: &str = r#"
pub fn greeting() -> String {
    let words: u32 = "not a number";
    words.to_string()
}
"#;

/// `shared` is outside the app's cargo workspace, so an edit there rebuilds the
/// bundle. `page_tag`'s body is what a web hot patch can carry.
fn app_rs(tag: &str) -> String {
    format!(
        r#"
use runtime_core::{{component, signal, ui, Element}};

fn page_tag() -> String {{
    let parts = ["tag", "{tag}"];
    parts.join(" ")
}}

#[component]
fn Root() -> Element {{
    let count = signal(0i32);
    let bump = move || count.set(count.get() + 1);
    ui! {{
        view {{
            text {{ move || format!("count = {{}}", count.get()) }}
            button(label = "+1".to_string(), on_click = bump)
            text {{ move || format!("page says {{}}", shared::greeting()) }}
            text {{ move || page_tag() }}
        }}
    }}
}}

pub fn app() -> Element {{
    ui! {{ Root() }}
}}
"#
    )
}

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
  <head><meta charset="utf-8" /><base href="/" /><title>session check e2e</title></head>
  <body>
    <div id="app"></div>
    <script type="module">
      import init from "/pkg/dev_session_check_e2e.js";
      init();
    </script>
  </body>
</html>
"#;

const SERVER_MAIN: &str = r#"
use axum::http::{header, StatusCode, Uri};
use axum::response::IntoResponse;

fn hello() -> String {
    format!("api / {}", shared::greeting())
}

async fn bundle(uri: Uri) -> axum::response::Response {
    let dist = std::path::PathBuf::from(std::env::var("WEB_DIST").expect("WEB_DIST"));
    let rel = uri.path().trim_start_matches('/');
    let path = if rel.is_empty() || rel.contains("..") || !dist.join(rel).is_file() {
        dist.join("index.html")
    } else {
        dist.join(rel)
    };
    let kind = match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html",
        Some("js") => "text/javascript",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css",
        _ => "application/octet-stream",
    };
    match std::fs::read(&path) {
        Ok(bytes) => ([(header::CONTENT_TYPE, kind)], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[tokio::main]
async fn main() {
    let port = std::env::var("PORT").expect("PORT");
    let app = server::router()
        .route("/api/hello", axum::routing::get(|| async { hello() }))
        .route("/api/pid", axum::routing::get(|| async { std::process::id().to_string() }))
        .fallback(bundle);
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await.unwrap();
    println!("fixture-server listening on {port}");
    axum::serve(listener, app).await.unwrap();
}
"#;

fn app_manifest(repo: &Path, config: &str) -> String {
    let dep = |p: &str| repo.join(p).display().to_string();
    format!(
        r#"[package]
name = "dev-session-check-e2e"
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
name = "Dev Session Check E2E"
bundle_id = "com.example.dev_session_check_e2e"
version = "0.0.1"
targets = ["web"]
server_manifest = "../server/Cargo.toml"
server_bin = "fixture-server"
{config}
[workspace]
"#,
        idealyst = dep("crates/idealyst"),
        core = dep("crates/runtime/core"),
        vocab = dep("crates/runtime/vocabulary"),
        scene = dep("crates/runtime/scene"),
    )
}

/// Write only what differs: an unchanged file keeps its mtime, so a test that
/// follows another finds its builds warm.
fn put(path: &Path, text: &str) {
    if std::fs::read_to_string(path).ok().as_deref() != Some(text) {
        std::fs::write(path, text).unwrap();
    }
}

fn materialize(root: &Path, repo: &Path, config: &str) {
    for d in ["app/src", "server/src", "shared/src", "shared/tests"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    // CrewForge's shape: the server and the crates it tests are members of one
    // workspace, and the app has its own. Cargo resolves features over selected
    // MEMBERS, so a member selected alone resolves differently: `shared` turns
    // on fewer features of tokio than the server does.
    put(&root.join("Cargo.toml"), "[workspace]\nresolver = \"2\"\nmembers = [\"server\", \"shared\"]\nexclude = [\"app\"]\n");
    put(
        &root.join("shared/Cargo.toml"),
        "[package]\nname = \"shared\"\nversion = \"0.0.1\"\nedition = \"2021\"\npublish = false\n\n\
         [dependencies]\ntokio = { version = \"1\", default-features = false, features = [\"sync\"] }\n",
    );
    put(&root.join("shared/src/lib.rs"), &shared("v1"));
    put(&root.join("shared/tests/greets.rs"), "#[test]\nfn greets() {\n    assert!(!shared::greeting().is_empty());\n}\n");
    put(&root.join("app/Cargo.toml"), &app_manifest(repo, config));
    put(&root.join("app/src/lib.rs"), LIB_RS);
    put(&root.join("app/src/app.rs"), &app_rs("t1"));
    put(&root.join("app/src/main.rs"), "idealyst::entry!(dev_session_check_e2e);\n");
    put(&root.join("app/index.html"), INDEX_HTML);
    if !root.join("app/Cargo.lock").exists() {
        std::fs::copy(repo.join("Cargo.lock"), root.join("app/Cargo.lock")).unwrap();
    }
    put(
        &root.join("server/Cargo.toml"),
        &format!(
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
"#,
            server = repo.join("crates/api/server").display(),
        ),
    );
    put(&root.join("server/src/main.rs"), SERVER_MAIN);
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("dev_session_check_e2e")
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Session {
    child: Child,
    log: Arc<Mutex<String>>,
    events: PathBuf,
    port: u16,
    app: PathBuf,
}

impl Drop for Session {
    fn drop(&mut self) {
        // SIGINT first: the CLI's handler takes the server tree down.
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGINT);
        }
        std::thread::sleep(Duration::from_millis(1500));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn seq(e: &Value) -> u64 {
    e["seq"].as_u64().unwrap_or(0)
}

impl Session {
    fn events(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.events)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn last_seq(&self) -> u64 {
        self.events().last().map(seq).unwrap_or(0)
    }

    fn wait_event(&self, after: u64, budget: Duration, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + budget;
        loop {
            if let Some(e) = self.events().into_iter().find(|e| seq(e) > after && pred(e)) {
                return e;
            }
            assert!(
                Instant::now() < deadline,
                "no {what} within {budget:?}. Events since {after}:\n{}\nLog tail:\n{}",
                self.summary(after),
                tail(&self.log.lock().unwrap()),
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn summary(&self, after: u64) -> String {
        self.events()
            .iter()
            .filter(|e| seq(e) > after && !matches!(e["type"].as_str(), Some("output" | "cargo_progress")))
            .map(|e| e.to_string().chars().take(260).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Event types between two points, for one target.
    fn kinds(&self, after: u64, target: &str) -> Vec<String> {
        self.events()
            .iter()
            .filter(|e| seq(e) > after && e["target"] == target)
            .filter_map(|e| e["type"].as_str().map(String::from))
            .collect()
    }

    /// The units cargo compiled for a target's builds since `after`, from the
    /// session's own events (a check reports its own in its report).
    fn units_built(&self, after: u64, target: &str) -> Vec<String> {
        self.events()
            .iter()
            .filter(|e| seq(e) > after && e["type"] == "unit_built" && e["target"] == target)
            .map(|e| format!("{} ({})", e["package"].as_str().unwrap_or("?"), e["kind"].as_str().unwrap_or("?")))
            .collect()
    }

    /// `idealyst check --session` in this session's directory.
    fn check(&self, extra: &[&str]) -> (bool, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_idealyst"))
            .current_dir(&self.app)
            .args(["check", "--session"])
            .args(extra)
            .output()
            .expect("run idealyst check");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        eprintln!("[check {extra:?}] exit {:?}\n{stderr}", out.status.code());
        (out.status.success(), stderr)
    }

    /// The reports of the check that finished after `after`, by target.
    fn reports(&self, after: u64) -> Vec<Value> {
        self.events().into_iter().filter(|e| seq(e) > after && e["type"] == "check_finished").collect()
    }

    fn pid(&self) -> String {
        http_get(self.port, "/api/pid").expect("the server answers /api/pid")
    }

    fn hello(&self) -> Option<String> {
        http_get(self.port, "/api/hello")
    }

    /// `POST /rebuild` through the session's registry entry: `rebuild_dev`.
    fn rebuild(&self) -> u64 {
        let registry = dev_events::sessions::registry_dir().expect("a registry dir");
        let entry = dev_events::sessions::read_all(&registry)
            .into_iter()
            .map(|(_, e)| e)
            .find(|e| e.pid == self.child.id())
            .expect("the session is registered");
        let (code, body) = dev_events::sessions::request(&entry, "POST", "/rebuild", Duration::from_secs(5)).unwrap();
        assert_eq!(code, 202, "{body}");
        serde_json::from_str::<Value>(&body).unwrap()["request"].as_u64().expect("a request number")
    }
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

/// Reads `Content-Length` bytes: Chrome's DevTools endpoint keeps the
/// connection open whatever the request says.
fn http_get(port: u16, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").ok()?;
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
    let mut body = vec![0u8; length?];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

fn wait_answer(session: &Session, want: &str, budget: Duration) {
    let started = Instant::now();
    loop {
        let got = session.hello();
        if got.as_deref() == Some(want) {
            return;
        }
        assert!(started.elapsed() < budget, "/api/hello still answers {got:?}, not {want:?}, after {budget:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn start(root: &Path, scratch: &Path, flags: &[&str]) -> Session {
    let log = Arc::new(Mutex::new(String::new()));
    let events_path = scratch.join(format!("events-{}.jsonl", free_port()));
    let port = free_port();
    let app = root.join("app");
    let mut child = Command::new(env!("CARGO_BIN_EXE_idealyst"))
        .current_dir(&app)
        .env("IDEALYST_TEST_DRIVER", "1")
        // A build every time: a resumed session runs no cargo, and these tests
        // count what cargo compiled.
        .env("IDEALYST_DEV_NO_RESUME", "1")
        .args(["dev", "--web", "--no-headless-client"])
        .args(flags)
        .args(["--port", &port.to_string()])
        .arg("--events-file")
        .arg(&events_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn idealyst dev");
    pump(child.stdout.take().unwrap(), log.clone());
    pump(child.stderr.take().unwrap(), log.clone());
    let session = Session { child, log, events: events_path, port, app };
    session.wait_event(0, COLD, "server_ready(full_stack)", |e| e["type"] == "server_ready" && e["kind"] == "full_stack");
    session.wait_event(0, COLD, "the web build", |e| e["type"] == "build_finished" && e["target"] == "web");
    session.wait_event(0, Duration::from_secs(120), "the web watcher", |e| {
        e["type"] == "watcher_ready" && e["target"] == "web"
    });
    session.wait_event(0, Duration::from_secs(120), "the server watcher", |e| {
        e["type"] == "watcher_ready" && e["target"] == "server"
    });
    session
}

fn browser() -> Option<String> {
    if let Ok(p) = std::env::var("IDEALYST_BROWSER") {
        return Path::new(&p).exists().then_some(p);
    }
    ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/usr/bin/google-chrome-stable", "/usr/bin/chromium"]
        .iter()
        .find(|p| Path::new(p).exists())
        .map(|p| p.to_string())
}

struct Chrome(Child);

impl Drop for Chrome {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A DevTools-protocol connection to the page.
struct Page {
    ws: tungstenite::WebSocket<TcpStream>,
    next_id: u64,
    _chrome: Chrome,
}

impl Page {
    fn open(port: u16, scratch: &Path) -> Page {
        let chrome = browser().expect("this test needs Chrome/Chromium, or IDEALYST_BROWSER pointing at one");
        let debug_port = free_port();
        let child = Command::new(&chrome)
            .args(["--headless=new", "--disable-gpu", "--no-sandbox", "--disable-dev-shm-usage", "--no-first-run"])
            .arg(format!("--remote-debugging-port={debug_port}"))
            .arg(format!("--user-data-dir={}", scratch.join(format!("chrome-{debug_port}")).display()))
            .arg(format!("http://127.0.0.1:{port}/"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn Chrome");
        let chrome = Chrome(child);
        let deadline = Instant::now() + Duration::from_secs(30);
        let ws_url = loop {
            let page = http_get(debug_port, "/json/list")
                .and_then(|body| serde_json::from_str::<Value>(&body).ok())
                .and_then(|list| {
                    list.as_array()?.iter().find_map(|t| {
                        let is_app = t["type"] == "page" && t["url"].as_str().is_some_and(|u| u.starts_with("http"));
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
        Page { ws, next_id: 1, _chrome: chrome }
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
            let tungstenite::Message::Text(text) = self.ws.read().expect("CDP read") else { continue };
            let v: Value = serde_json::from_str(&text).unwrap();
            if v["id"].as_u64() == Some(id) {
                return v["result"]["result"]["value"].clone();
            }
        }
    }

    fn text(&mut self) -> String {
        self.eval("document.body ? document.body.innerText : ''").as_str().unwrap_or_default().to_string()
    }

    fn wait_text(&mut self, needle: &str, budget: Duration) {
        let deadline = Instant::now() + budget;
        loop {
            let text = self.text();
            if text.contains(needle) {
                return;
            }
            assert!(Instant::now() < deadline, "the page never showed {needle:?}: {text:?}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Click +1 and leave a marker a reload would clear.
    fn make_state(&mut self) {
        self.wait_text("count = 0", Duration::from_secs(60));
        self.eval(
            "(() => { const el = [...document.querySelectorAll('button,[role=button]')].find(b => b.innerText.includes('+1')); el.click(); })()",
        );
        self.wait_text("count = 1", Duration::from_secs(10));
        self.eval("window.__session_check_marker = 'kept'; 1");
    }

    fn state_kept(&mut self) -> bool {
        self.eval("window.__session_check_marker") == "kept" && self.text().contains("count = 1")
    }
}

/// Every target's check report, by target; both targets must have reported.
fn both_reports(session: &Session, after: u64) -> (Value, Value) {
    let reports = session.reports(after);
    let of = |target: &str| {
        reports
            .iter()
            .find(|r| r["target"] == target)
            .cloned()
            .unwrap_or_else(|| panic!("no {target} report: {}", session.summary(after)))
    };
    (of("web"), of("server"))
}

fn compiled(report: &Value) -> Vec<String> {
    report["compiled"].as_array().unwrap().iter().filter_map(|u| u.as_str().map(String::from)).collect()
}

/// No event a status view reads as a build, a save being applied or a reload.
fn assert_nothing_applied(session: &Session, after: u64) {
    for target in ["web", "server"] {
        let kinds = session.kinds(after, target);
        for applied in [
            "build_started",
            "build_finished",
            "rebuild_taken",
            "server_ready",
            "server_down",
            "patch_built",
            "overlay_pushed",
            "cargo_progress",
            "diagnostic",
            "unit_built",
        ] {
            assert!(!kinds.iter().any(|k| k == applied), "{target}: a check emitted {applied}: {kinds:?}");
        }
    }
}

#[test]
#[ignore = "compiles the framework for wasm32 and the host and drives headless Chrome; run with --ignored"]
fn a_check_applies_nothing_and_the_next_rebuild_reuses_it() {
    let repo = framework_root();
    let root = fixture_root();
    materialize(&root, &repo, "");
    let scratch = tempfile::tempdir().unwrap();
    let session = start(&root, scratch.path(), &["--local", "--no-robot", "--no-watch"]);
    let mut page = Page::open(session.port, scratch.path());
    assert_eq!(session.hello().as_deref(), Some("api / greeting v1"));
    page.make_state();
    let first_pid = session.pid();

    // ── Criterion 1: warm, no edits: nothing compiles ────────────────────
    let before = session.last_seq();
    let started = Instant::now();
    let (ok, _) = session.check(&[]);
    assert!(ok);
    eprintln!("[e2e] warm check took {:?}", started.elapsed());
    let (web, server) = both_reports(&session, before);
    assert_eq!(compiled(&web), Vec::<String>::new(), "a warm web check compiled: {web}");
    assert_eq!(compiled(&server), Vec::<String>::new(), "a warm server check compiled: {server}");

    // ── Criterion 3: an edit is compiled, and nothing moves ──────────────
    let before = session.last_seq();
    std::fs::write(root.join("shared/src/lib.rs"), shared("v2")).unwrap();
    session.wait_event(before, Duration::from_secs(10), "the queued save", |e| {
        e["type"] == "change_detected" && e["target"] == "server"
    });
    let before = session.last_seq();
    let (ok, _) = session.check(&[]);
    assert!(ok);
    let (web, server) = both_reports(&session, before);
    assert!(compiled(&web).iter().any(|u| u.starts_with("shared ")), "{web}");
    assert!(compiled(&server).iter().any(|u| u.starts_with("shared ")), "{server}");
    assert_nothing_applied(&session, before);
    assert_eq!(session.pid(), first_pid, "a check restarted the server");
    assert_eq!(session.hello().as_deref(), Some("api / greeting v1"), "a check applied the edit to the server");
    assert!(page.state_kept(), "a check disturbed the page: {:?}", page.text());
    assert!(page.text().contains("page says greeting v1"), "{:?}", page.text());

    // ── Criterion 4: a compile error fails, and is located ───────────────
    let before = session.last_seq();
    let good = std::fs::read_to_string(root.join("shared/src/lib.rs")).unwrap();
    std::fs::write(root.join("shared/src/lib.rs"), SHARED_BROKEN).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let (ok, stderr) = session.check(&[]);
    assert!(!ok, "a failing check exited 0");
    assert!(stderr.contains("FAILED"), "{stderr}");
    assert!(stderr.contains("shared/src/lib.rs"), "{stderr}");
    let (web, server) = both_reports(&session, before);
    for report in [&web, &server] {
        assert_eq!(report["ok"], false, "{report}");
        let error = &report["errors"][0];
        assert!(error["file"].as_str().unwrap().ends_with("shared/src/lib.rs"), "{report}");
        assert_eq!(error["line"], 3, "{report}");
        assert!(error["message"].as_str().unwrap().contains("mismatched types"), "{report}");
    }
    assert_nothing_applied(&session, before);
    assert_eq!(session.pid(), first_pid);
    assert_eq!(session.hello().as_deref(), Some("api / greeting v1"));
    assert!(page.state_kept(), "a failed check disturbed the page: {:?}", page.text());
    assert!(!page.text().to_lowercase().contains("mismatched types"), "a check's error reached the page");
    std::fs::write(root.join("shared/src/lib.rs"), &good).unwrap();
    std::thread::sleep(Duration::from_millis(300));

    // ── Criteria 2 and 5: the real rebuild applies the held saves, and
    //    finds everything compiled ────────────────────────────────────────
    let (ok, _) = session.check(&[]);
    assert!(ok);
    let before = session.last_seq();
    let request = session.rebuild();
    assert_eq!(request, 1, "checks moved the rebuild requests' numbering");
    session.wait_event(before, Duration::from_secs(300), "the restarted server", |e| {
        e["type"] == "server_ready" && e["kind"] == "full_stack"
    });
    session.wait_event(before, Duration::from_secs(300), "the web rebuild", |e| {
        e["type"] == "build_finished" && e["target"] == "web"
    });
    assert!(
        session.events().iter().any(|e| seq(e) > before && e["type"] == "rebuild_taken" && e["request"] == 1),
        "{}",
        session.summary(before)
    );
    assert_eq!(session.units_built(before, "server"), Vec::<String>::new(), "the rebuild recompiled the server's crates");
    assert_eq!(session.units_built(before, "web"), Vec::<String>::new(), "the rebuild recompiled the web crates");
    let server_outcome = session.wait_event(before, Duration::from_secs(10), "the server's outcome", |e| {
        e["type"] == "build_finished" && e["target"] == "server"
    });
    assert_eq!(server_outcome["outcome"], "reloaded", "unchanged against the last build, not what ran: {server_outcome}");
    wait_answer(&session, "api / greeting v2", Duration::from_secs(30));
    assert_ne!(session.pid(), first_pid, "the rebuild did not restart the server");
    page.wait_text("page says greeting v2", Duration::from_secs(60));
    assert_ne!(page.eval("window.__session_check_marker"), "kept", "the page did not reload onto the new bundle");
    drop(page);
    drop(session);
    materialize(&root, &repo, "");
}

#[test]
#[ignore = "compiles the framework for wasm32 and the host and drives headless Chrome; run with --ignored"]
fn a_hot_patch_after_a_check_reaches_the_running_program() {
    let repo = framework_root();
    let root = fixture_root();
    materialize(&root, &repo, "");
    let scratch = tempfile::tempdir().unwrap();
    let session = start(&root, scratch.path(), &["--local", "--no-robot", "--experimental-server-patch"]);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !session.log.lock().unwrap().contains("hot-patch agent connected") {
        assert!(Instant::now() < deadline, "the server's hot-patch agent never connected");
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut page = Page::open(session.port, scratch.path());
    page.make_state();
    let first_pid = session.pid();

    let web_patch = |after: u64| {
        let e = session.wait_event(after, Duration::from_secs(300), "the web patch", |e| {
            e["target"] == "web" && matches!(e["type"].as_str(), Some("patch_built" | "patch_failed" | "build_started"))
        });
        assert_eq!(e["type"], "patch_built", "{e}\n{}", session.summary(after));
    };
    let server_patch = |after: u64| {
        let e = session.wait_event(after, Duration::from_secs(300), "the server's patch", |e| {
            e["target"] == "server" && matches!(e["type"].as_str(), Some("patch_built" | "patch_failed" | "build_started"))
        });
        assert_eq!(e["type"], "patch_built", "the base no longer describes the running server: {e}\n{}", session.summary(after));
    };

    // Body edits patch both sides, and the disk now differs from each base.
    let before = session.last_seq();
    std::fs::write(root.join("shared/src/lib.rs"), shared("v2")).unwrap();
    server_patch(before);
    wait_answer(&session, "api / greeting v2", Duration::from_secs(10));
    // The web side rebuilds for `shared` (see `app_rs`), and reloads.
    page.wait_text("page says greeting v2", Duration::from_secs(300));
    session.wait_event(before, Duration::from_secs(300), "the web rebuild", |e| {
        e["target"] == "web" && e["type"] == "build_finished"
    });
    page.make_state();
    let before = session.last_seq();
    std::fs::write(root.join("app/src/app.rs"), app_rs("t2")).unwrap();
    web_patch(before);
    page.wait_text("tag t2", Duration::from_secs(60));
    assert!(page.state_kept(), "{:?}", page.text());

    // The check compiles those edits: rlibs rewritten, the server's binary
    // relinked on disk, cargo's wasm rewritten. The test-harness check goes
    // through the same queue, and must not touch a capture a patch replays.
    let before = session.last_seq();
    let (ok, _) = session.check(&[]);
    assert!(ok);
    let (web, server) = both_reports(&session, before);
    assert!(compiled(&server).iter().any(|u| u.starts_with("fixture-server ")), "the binary relinked: {server}");
    assert!(compiled(&web).iter().any(|u| u.starts_with("dev-session-check-e2e ")), "{web}");
    let (ok, _) = session.check(&["--tests", "-p", "shared"]);
    assert!(ok);
    assert_nothing_applied(&session, before);
    assert_eq!(session.pid(), first_pid);
    assert!(page.state_kept(), "{:?}", page.text());

    // Criterion 6: further body edits patch the RUNNING page and server.
    let before = session.last_seq();
    std::fs::write(root.join("app/src/app.rs"), app_rs("t3")).unwrap();
    web_patch(before);
    page.wait_text("tag t3", Duration::from_secs(60));
    assert!(page.state_kept(), "the patch reloaded the page: {:?}", page.text());
    assert!(page.text().contains("page says greeting v2"), "{:?}", page.text());

    let before = session.last_seq();
    std::fs::write(root.join("shared/src/lib.rs"), shared("v3")).unwrap();
    server_patch(before);
    wait_answer(&session, "api / greeting v3", Duration::from_secs(10));
    assert_eq!(session.pid(), first_pid, "the server was respawned, not patched");
    drop(page);
    drop(session);
    materialize(&root, &repo, "");
}

/// A save and its `POST /rebuild` with no gap, twice, as an agent's edit and
/// `rebuild_dev` do. The request reaches each watcher before the save does. A
/// request answered from what the watcher has reported forces a build; the next
/// releases that first save, which the build compiled, answers "no change that
/// reaches the program", and leaves the new edit queued, never applied.
#[test]
#[ignore = "compiles the framework for wasm32 and the host and drives headless Chrome; run with --ignored"]
fn regression_a_save_with_no_gap_before_its_request_is_applied() {
    let repo = framework_root();
    let root = fixture_root();
    materialize(&root, &repo, "");
    let scratch = tempfile::tempdir().unwrap();
    let session = start(&root, scratch.path(), &["--local", "--no-robot", "--no-watch", "--experimental-server-patch"]);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !session.log.lock().unwrap().contains("hot-patch agent connected") {
        assert!(Instant::now() < deadline, "the server's hot-patch agent never connected");
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut page = Page::open(session.port, scratch.path());
    page.wait_text("count = 0", Duration::from_secs(60));

    let before = session.last_seq();
    std::fs::write(root.join("shared/src/lib.rs"), shared("v2")).unwrap();
    session.rebuild();
    let server = session.wait_event(before, Duration::from_secs(300), "the server's answer", |e| {
        e["target"] == "server" && matches!(e["type"].as_str(), Some("patch_built" | "patch_failed" | "build_started"))
    });
    assert_eq!(server["type"], "patch_built", "the request did not offer the save to the patch: {}", session.summary(before));
    wait_answer(&session, "api / greeting v2", Duration::from_secs(30));
    page.wait_text("page says greeting v2", Duration::from_secs(300));
    session.wait_event(before, Duration::from_secs(300), "the web rebuild", |e| {
        e["target"] == "web" && e["type"] == "build_finished"
    });
    page.make_state();

    // Other files than the first request's: re-reading what the watcher
    // reported cannot find these.
    let before = session.last_seq();
    std::fs::write(root.join("app/src/app.rs"), app_rs("t2")).unwrap();
    std::fs::write(root.join("server/src/main.rs"), SERVER_MAIN.replace("api / {}", "api two / {}")).unwrap();
    session.rebuild();
    page.wait_text("tag t2", Duration::from_secs(300));
    assert!(page.state_kept(), "the web save did not hot patch: {:?}", page.text());
    wait_answer(&session, "api two / greeting v2", Duration::from_secs(300));
    for target in ["web", "server"] {
        let unchanged = session.events().into_iter().find(|e| {
            seq(e) > before && e["target"] == target && e["type"] == "decided" && e["decision"]["tier"] == "unchanged"
        });
        assert_eq!(unchanged, None, "{target} answered no change: {}", session.summary(before));
    }

    // Each late event is a save a request already took: nothing is queued, so
    // the status views have no row waiting on a request.
    std::thread::sleep(Duration::from_secs(3));
    let events = session.events();
    let last_taken = events.iter().rposition(|e| e["type"] == "rebuild_taken").expect("the requests were taken");
    for target in ["web", "server"] {
        let reports = events[last_taken..]
            .iter()
            .filter(|e| e["type"] == "change_detected" && e["target"] == target)
            .count();
        assert!(reports <= 1, "{target} queued a save a request had taken: {}", session.summary(before));
    }
    drop(page);
    drop(session);
    materialize(&root, &repo, "");
}

#[test]
#[ignore = "compiles the framework for wasm32 and the host; run with --ignored"]
fn a_test_harness_check_compiles_only_the_harness() {
    let repo = framework_root();
    let root = fixture_root();
    materialize(&root, &repo, "");
    let scratch = tempfile::tempdir().unwrap();
    let session = start(&root, scratch.path(), &["--local", "--no-robot"]);

    // Criterion 7. Its harness may already exist from an earlier run of this
    // test, so make it stale; `shared`'s dependents are the session's own
    // units and must not be compiled for it.
    let lib = root.join("shared/src/lib.rs");
    let text = std::fs::read_to_string(&lib).unwrap();
    let before = session.last_seq();
    std::fs::write(&lib, format!("{text}\n// a comment, to make the harness stale\n")).unwrap();
    // The session rebuilds for the save, which leaves its own units fresh and
    // only the harness stale.
    session.wait_event(before, Duration::from_secs(300), "the restarted server", |e| {
        e["type"] == "server_ready" && e["kind"] == "full_stack"
    });
    session.wait_event(before, Duration::from_secs(300), "the web rebuild", |e| {
        e["target"] == "web" && e["type"] == "build_finished"
    });
    let first_pid = session.pid();
    let before = session.last_seq();
    let started = Instant::now();
    let (ok, stderr) = session.check(&["--tests", "-p", "shared"]);
    assert!(ok, "{stderr}");
    eprintln!("[e2e] harness check took {:?}", started.elapsed());
    let reports = session.reports(before);
    assert_eq!(reports.len(), 1, "only the server's host build compiles tests: {reports:?}");
    let server = &reports[0];
    assert_eq!(server["target"], "server");
    let units = compiled(server);
    assert!(!units.is_empty(), "{server}");
    assert!(units.iter().all(|u| u.starts_with("shared (")), "a dependency was compiled: {units:?}");
    assert!(stderr.contains("not checked: web"), "{stderr}");
    assert!(!stderr.contains("other unit(s)"), "{stderr}");

    // Warm, it compiles nothing.
    let before = session.last_seq();
    let (ok, _) = session.check(&["--tests", "-p", "shared"]);
    assert!(ok);
    assert_eq!(compiled(&session.reports(before)[0]), Vec::<String>::new());

    // A crate only the web bundle uses has nothing to reuse here: refused in
    // words, as a failing status.
    let (ok, stderr) = session.check(&["--tests", "-p", "dev-session-check-e2e"]);
    assert!(!ok, "{stderr}");
    assert!(stderr.contains("not a local package"), "{stderr}");
    assert_eq!(session.pid(), first_pid);
    drop(session);
    materialize(&root, &repo, "");
}

/// The web target dir a session's log names.
fn target_dir(session: &Session) -> String {
    let log = session.log.lock().unwrap();
    let line = log.lines().find(|l| l.contains("target dir: ") && l.contains("idealyst-web-")).expect("a target dir line");
    let at = line.find("target dir: ").unwrap() + "target dir: ".len();
    line[at..].split(' ').next().unwrap().to_string()
}

#[test]
#[ignore = "compiles the framework for wasm32 and the host; run with --ignored"]
fn the_project_configuration_chooses_the_build() {
    let repo = framework_root();
    let root = fixture_root();
    let scratch = tempfile::tempdir().unwrap();

    // The flagged command, with no configuration.
    materialize(&root, &repo, "");
    let flagged = start(&root, scratch.path(), &["--local", "--no-robot", "--debuginfo", "line-tables", "--dev-opt", "optimized"]);
    let flagged_dir = target_dir(&flagged);
    assert!(
        !flagged.log.lock().unwrap().contains("differs from this project's configuration"),
        "no configuration, no warnings"
    );
    drop(flagged);

    // Criterion 8: the same settings declared, and no build flags.
    let config = "\n[package.metadata.idealyst.dev]\nlocal = true\nno_robot = true\nfeatures = []\ndebuginfo = \"line-tables-only\"\ndev_opt = \"optimized\"\nexperimental_build = false\n";
    materialize(&root, &repo, config);
    let configured = start(&root, scratch.path(), &[]);
    assert_eq!(target_dir(&configured), flagged_dir, "the configuration moved the build");
    let log = configured.log.lock().unwrap().clone();
    assert!(!log.contains("differs from this project's configuration"), "{}", tail(&log));
    // Only the app and what a changed manifest touches: no dependency.
    let units = configured.units_built(0, "web");
    assert!(units.iter().all(|u| u.starts_with("dev-session-check-e2e ")), "{units:?}");
    assert_eq!(http_get(configured.port, "/api/hello").as_deref(), Some("api / greeting v1"));
    drop(configured);

    // Criterion 9: a behaviour flag says nothing.
    let behaviour = start(&root, scratch.path(), &["--no-watch"]);
    assert!(!behaviour.log.lock().unwrap().contains("differs from this project's configuration"));
    drop(behaviour);

    // Criterion 9: a differing flag warns before the first build, and names
    // the consequence. Criterion 10: its session retires nothing configured.
    let oneoff = start(&root, scratch.path(), &["--debuginfo", "full"]);
    let events = oneoff.events();
    let warned = events
        .iter()
        .position(|e| e["type"] == "warning" && e["message"].as_str().is_some_and(|m| m.contains("`--debuginfo full` differs")))
        .unwrap_or_else(|| panic!("no warning: {}", oneoff.summary(0)));
    let building = events.iter().position(|e| e["type"] == "build_started").expect("a build");
    assert!(warned < building, "the warning came after the build started");
    let message = events[warned]["message"].as_str().unwrap();
    for needle in ["`line-tables-only`", "full dependency rebuild", "[package.metadata.idealyst.dev]", "`debuginfo`"] {
        assert!(message.contains(needle), "{message}");
    }
    let oneoff_dir = target_dir(&oneoff);
    assert_ne!(oneoff_dir, flagged_dir, "the override built in the configured directory");
    drop(oneoff);
    assert!(Path::new(&flagged_dir).is_dir(), "the one-off session retired the configured build");

    // Criterion 10: the next normal session finds it warm, and the override's
    // own directory is retired on the way back.
    let back = start(&root, scratch.path(), &[]);
    assert_eq!(target_dir(&back), flagged_dir);
    assert_eq!(back.units_built(0, "web"), Vec::<String>::new(), "the configured build's cache did not survive");
    assert!(!Path::new(&oneoff_dir).is_dir(), "the one-off directory outlived its session");
    assert!(
        !back.log.lock().unwrap().contains("wasm unchanged since the last build"),
        "the override's bundle was served as the configuration's"
    );
    drop(back);
    materialize(&root, &repo, "");
}

fn idealyst_check(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_idealyst")).current_dir(dir).arg("check").args(args).output().expect("run idealyst check")
}

/// A crate that compiles only with `beta` on: a host `idealyst check` of it
/// passes only if the table's `features` reached cargo.
fn tiny_project(dir: &Path, table: &str) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "tiny"
version = "0.0.1"
edition = "2021"
publish = false

[features]
beta = []

[package.metadata.idealyst.app]
name = "Tiny"
bundle_id = "com.example.tiny"
version = "0.0.1"
targets = ["terminal"]
{table}
[workspace]
"#
        ),
    )
    .unwrap();
    std::fs::write(dir.join("src/lib.rs"), "#[cfg(not(feature = \"beta\"))]\ncompile_error!(\"beta is off\");\n").unwrap();
}

/// Feature 4: `idealyst check` reads the same table. Without one it is the
/// same `cargo check` it always was.
#[test]
fn a_plain_check_reads_the_configured_features() {
    let dir = tempfile::tempdir().unwrap();
    tiny_project(dir.path(), "");
    let out = idealyst_check(dir.path(), &[]);
    assert!(!out.status.success(), "no table, no feature: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("beta is off"));
    tiny_project(dir.path(), "\n[package.metadata.idealyst.dev]\nfeatures = [\"beta\"]\n");
    let out = idealyst_check(dir.path(), &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // A misspelled key is an error at the start, naming the table.
    tiny_project(dir.path(), "\n[package.metadata.idealyst.dev]\nfeature = [\"beta\"]\n");
    let out = idealyst_check(dir.path(), &[]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("[package.metadata.idealyst.dev]"));
}

/// `--session`'s command line: `-p` means the crate whose tests `--tests`
/// compiles, and nothing else. A targeted check of one crate does not exist,
/// so it must not parse into a whole-session one. And with no session the
/// command says what to do instead of checking something else.
#[test]
fn the_session_flags_refuse_what_they_do_not_do() {
    let dir = tempfile::tempdir().unwrap();
    tiny_project(dir.path(), "");
    for args in [
        &["--session", "-p", "tiny"][..],
        &["--tests", "-p", "tiny"],
        &["--session", "--tests"],
        &["--session", "--release"],
        &["--session", "--platform", "web"],
    ] {
        let out = idealyst_check(dir.path(), args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let out = idealyst_check(dir.path(), &["--session"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no `idealyst dev` session is running"), "{stderr}");
}
