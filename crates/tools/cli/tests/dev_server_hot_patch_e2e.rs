//! End-to-end: in a FULL-STACK `idealyst dev --web --local` session, a
//! body edit in a crate the server is built from is patched into the
//! RUNNING server — no rebuild, no respawn — and an edit a patch cannot
//! carry still rebuilds and respawns it.
//!
//! The fixture is CrewForge's shape in miniature (as `dev_fullstack_e2e`):
//!
//! ```text
//! <tmp>/dev_server_hot_patch_e2e/
//!   app/     the web app (server_manifest = "../server/Cargo.toml")
//!   server/  a standalone axum server on `server::router()`
//!   shared/  a crate both depend on
//! ```
//!
//! What it asserts, in order:
//!
//! 1. a body edit in `shared/` is a server `patch_built` (the web side
//!    hot patches its own copy); `/api/hello` answers with the new body
//!    from the SAME server process (`/api/pid`), within a time bound,
//!    and the server's watcher never ran a build for it;
//! 2. a shape edit in `shared/` (a signature) is decided `rebuild`: the
//!    server is rebuilt and respawned (a new pid) and answers with the
//!    new code;
//! 3. a body edit after that rebuild patches the NEW process — the
//!    session re-read the relinked binary.
//!
//! And without `--experimental-build` or `--experimental-server-patch`
//! the tier is off: no agent loads,
//! and the same body edit rebuilds and respawns the server.
//!
//! `#[ignore]`d: it compiles the framework for wasm32 and the host. arm64
//! macOS and Linux, the hosts the server tier arms on:
//!
//! ```text
//! cargo test -p idealyst-cli --test dev_server_hot_patch_e2e -- --ignored --nocapture
//! ```
//!
//! On a Mac, the Linux leg runs in an arm64 dev container with the wasm
//! target added (`rustup target add wasm32-unknown-unknown`). With
//! `CARGO_TARGET_DIR` outside the repo the fixture cannot see the repo's
//! `.cargo/config.toml`, so name the registry in the environment too:
//! `CARGO_REGISTRIES_IDEALYST_INDEX=sparse+https://crates.idealyst.io/index/`.
//! Mount a container-local volume over the repo's `target/` as well: a
//! Docker Desktop bind mount of a macOS folder makes cargo recompile the
//! server on `cargo run`, and the session then refuses to patch a process
//! that is not its last build.

#![cfg(all(any(target_os = "macos", target_os = "linux"), target_arch = "aarch64"))]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A patch's save-to-answer time on this fixture, with room for a loaded
/// machine: the measured figure is ~1–2 s, a rebuild + respawn ~5–15 s.
const PATCH_BUDGET: Duration = Duration::from_secs(20);

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(3).expect("workspace root").to_path_buf()
}

fn shared(greeting: &str) -> String {
    format!(
        r#"
pub fn greeting() -> String {{
    let words = ["greeting", "{greeting}"];
    words.join(" ")
}}
"#
    )
}

/// A signature change: `greeting` takes an argument. The server's call
/// site passes one (see [`server_main`]).
fn shared_shape(greeting: &str) -> String {
    format!(
        r#"
pub fn greeting() -> String {{
    greeting_for("{greeting}")
}}

pub fn greeting_for(who: &str) -> String {{
    format!("greeting {{who}}")
}}
"#
    )
}

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
  <head><meta charset="utf-8" /><base href="/" /><title>server hot patch e2e</title></head>
  <body>
    <div id="app"></div>
    <script type="module">
      import init from "/pkg/dev_server_hot_patch_e2e.js";
      init();
    </script>
  </body>
</html>
"#;

const SERVER_MAIN: &str = r#"
fn hello() -> String {
    format!("api / {}", shared::greeting())
}

#[tokio::main]
async fn main() {
    let port = std::env::var("PORT").expect("PORT");
    let app = server::router()
        .route("/api/hello", axum::routing::get(|| async { hello() }))
        .route("/api/pid", axum::routing::get(|| async { std::process::id().to_string() }));
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await.unwrap();
    println!("fixture-server listening on {port}");
    axum::serve(listener, app).await.unwrap();
}
"#;

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
    std::fs::write(root.join("shared/src/lib.rs"), shared("v1")).unwrap();
    std::fs::write(
        root.join("app/Cargo.toml"),
        format!(
            r#"[package]
name = "dev-server-hot-patch-e2e"
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
name = "Dev Server Hot Patch E2E"
bundle_id = "com.example.dev_server_hot_patch_e2e"
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
    std::fs::write(root.join("app/src/main.rs"), "idealyst::entry!(dev_server_hot_patch_e2e);\n").unwrap();
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
    std::fs::write(root.join("server/src/main.rs"), SERVER_MAIN).unwrap();
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Session {
    child: Child,
    log: Arc<Mutex<String>>,
    events: PathBuf,
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

impl Session {
    fn events(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.events)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn last_seq(&self) -> u64 {
        self.events().last().and_then(|e| e["seq"].as_u64()).unwrap_or(0)
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
                summary(&self.events(), after),
                tail(&self.log.lock().unwrap()),
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// The server's events since `after`, by type.
    fn server_kinds(&self, after: u64, until: u64) -> Vec<String> {
        self.events()
            .iter()
            .filter(|e| seq(e) > after && seq(e) <= until && e["target"] == "server")
            .filter_map(|e| e["type"].as_str().map(String::from))
            .filter(|t| !matches!(t.as_str(), "output" | "cargo_progress" | "log"))
            .collect()
    }
}

fn seq(e: &Value) -> u64 {
    e["seq"].as_u64().unwrap_or(0)
}

fn summary(events: &[Value], after: u64) -> String {
    events
        .iter()
        .filter(|e| seq(e) > after)
        .filter(|e| !matches!(e["type"].as_str(), Some("output" | "cargo_progress")))
        .map(|e| e.to_string().chars().take(260).collect::<String>())
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
    let mut body = vec![0u8; length?];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

/// Poll `path` until it answers `want`, returning how long that took.
fn wait_answer(port: u16, path: &str, want: &str, budget: Duration) -> Duration {
    let started = Instant::now();
    loop {
        let got = http_get(port, path);
        if got.as_deref() == Some(want) {
            return started.elapsed();
        }
        assert!(started.elapsed() < budget, "{path} still answers {got:?}, not {want:?}, after {budget:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn pid(port: u16) -> String {
    http_get(port, "/api/pid").expect("the server answers /api/pid")
}

/// Both tests build the same fixture directory (a fresh path is a cold
/// build of both sides), so they take turns.
static FIXTURE: Mutex<()> = Mutex::new(());

/// Start `idealyst dev` on the fixture with `flags` added, and wait for
/// the full-stack server and the first web build.
fn start(root: &Path, scratch: &Path, port: u16, flags: &[&str]) -> Session {
    let log = Arc::new(Mutex::new(String::new()));
    let events_path = scratch.join("events.jsonl");
    let mut child = Command::new(env!("CARGO_BIN_EXE_idealyst"))
        .current_dir(root.join("app"))
        .env("IDEALYST_TEST_DRIVER", "1")
        .args(["dev", "--web", "--local", "--no-robot", "--no-headless-client"])
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
    let session = Session { child, log, events: events_path };
    session.wait_event(0, Duration::from_secs(1800), "server_ready(full_stack)", |e| {
        e["type"] == "server_ready" && e["kind"] == "full_stack"
    });
    session.wait_event(0, Duration::from_secs(1800), "the web build", |e| {
        e["type"] == "build_finished" && e["target"] == "web"
    });
    session
}

#[test]
#[ignore = "compiles the framework for wasm32 and the host; run with --ignored"]
fn a_body_edit_patches_the_running_server_and_a_shape_edit_respawns_it() {
    let _turn = FIXTURE.lock().unwrap_or_else(|e| e.into_inner());
    let repo = repo_root();
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dev_server_hot_patch_e2e");
    materialize(&root, &repo);
    let scratch = tempfile::tempdir().unwrap();
    let port = free_port();
    let session = start(&root, scratch.path(), port, &["--experimental-server-patch"]);
    let log = session.log.clone();
    // The server loaded the agent and reported in.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !log.lock().unwrap().contains("hot-patch agent connected") {
        assert!(Instant::now() < deadline, "the server's hot-patch agent never connected:\n{}", tail(&log.lock().unwrap()));
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(http_get(port, "/api/hello").as_deref(), Some("api / greeting v1"));
    let first_pid = pid(port);

    // ── 1. a body edit in the shared crate: patched in place ─────────
    let before = session.last_seq();
    let saved = Instant::now();
    std::fs::write(root.join("shared/src/lib.rs"), shared("v2")).unwrap();
    let patched = session.wait_event(before, Duration::from_secs(600), "the server's patch", |e| {
        e["target"] == "server" && matches!(e["type"].as_str(), Some("patch_built" | "patch_failed" | "build_started"))
    });
    assert_eq!(
        patched["type"], "patch_built",
        "the server was not patched: {patched}\n{}",
        summary(&session.events(), before)
    );
    let took = saved.elapsed() + wait_answer(port, "/api/hello", "api / greeting v2", Duration::from_secs(5));
    eprintln!("[e2e] server patched: save → new answer in {took:?} ({} fns redirected)", patched["redirected"]);
    assert!(took < PATCH_BUDGET, "the patch took {took:?}");
    assert_eq!(pid(port), first_pid, "the server was respawned, not patched");
    let kinds = session.server_kinds(before, seq(&patched));
    assert!(!kinds.iter().any(|k| k == "build_started"), "the server's watcher built: {kinds:?}");
    assert!(patched["crates"].as_array().is_some_and(|c| c.len() == 1 && c[0]["name"] == "shared"), "{patched}");

    // ── 2. a signature edit: rebuilt and respawned ───────────────────
    let before = session.last_seq();
    std::fs::write(root.join("shared/src/lib.rs"), shared_shape("v3")).unwrap();
    let decided = session.wait_event(before, Duration::from_secs(60), "the server's decision", |e| {
        e["target"] == "server" && e["type"] == "decided"
    });
    assert_eq!(decided["decision"]["tier"], "rebuild", "{decided}");
    session.wait_event(before, Duration::from_secs(600), "the respawned server", |e| {
        e["type"] == "server_ready" && e["kind"] == "full_stack"
    });
    wait_answer(port, "/api/hello", "api / greeting v3", Duration::from_secs(30));
    let second_pid = pid(port);
    assert_ne!(second_pid, first_pid, "a shape edit must respawn the server");

    // ── 3. a body edit after the rebuild patches the new process ─────
    let deadline = Instant::now() + Duration::from_secs(30);
    while log.lock().unwrap().matches("hot-patch agent connected").count() < 2 {
        assert!(Instant::now() < deadline, "the respawned server's agent never connected");
        std::thread::sleep(Duration::from_millis(100));
    }
    let before = session.last_seq();
    std::fs::write(root.join("shared/src/lib.rs"), shared_shape("v4")).unwrap();
    let patched = session.wait_event(before, Duration::from_secs(600), "the second patch", |e| {
        e["target"] == "server" && matches!(e["type"].as_str(), Some("patch_built" | "patch_failed" | "build_started"))
    });
    assert_eq!(patched["type"], "patch_built", "{patched}\n{}", summary(&session.events(), before));
    wait_answer(port, "/api/hello", "api / greeting v4", Duration::from_secs(5));
    assert_eq!(pid(port), second_pid);

    drop(session);
    materialize(&root, &repo);
}

/// The tier's off switch is the flag: without one that arms it the
/// session never arms it — no agent is loaded into the server, and the
/// same body edit that is patched above rebuilds and respawns the server
/// as `idealyst dev` always did. This is what makes the tier safe to pull.
#[test]
#[ignore = "compiles the framework for wasm32 and the host; run with --ignored"]
fn without_experimental_build_a_body_edit_rebuilds_and_respawns_the_server() {
    let _turn = FIXTURE.lock().unwrap_or_else(|e| e.into_inner());
    let repo = repo_root();
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dev_server_hot_patch_e2e");
    materialize(&root, &repo);
    let scratch = tempfile::tempdir().unwrap();
    let port = free_port();
    let session = start(&root, scratch.path(), port, &[]);
    assert_eq!(http_get(port, "/api/hello").as_deref(), Some("api / greeting v1"));
    let first_pid = pid(port);

    let before = session.last_seq();
    std::fs::write(root.join("shared/src/lib.rs"), shared("v2")).unwrap();
    session.wait_event(before, Duration::from_secs(600), "the respawned server", |e| {
        e["type"] == "server_ready" && e["kind"] == "full_stack"
    });
    wait_answer(port, "/api/hello", "api / greeting v2", Duration::from_secs(30));
    assert_ne!(pid(port), first_pid, "without the flag a body edit must respawn the server");
    let kinds = session.server_kinds(before, session.last_seq());
    assert!(kinds.iter().any(|k| k == "build_started"), "the server was not rebuilt: {kinds:?}");
    assert!(
        !kinds.iter().any(|k| matches!(k.as_str(), "patch_built" | "patch_failed" | "patch_step")),
        "the server tier ran without the flag: {kinds:?}"
    );
    assert!(
        !session.log.lock().unwrap().contains("hot-patch agent connected"),
        "the server loaded the hot-patch agent without the flag"
    );

    drop(session);
    materialize(&root, &repo);
}
