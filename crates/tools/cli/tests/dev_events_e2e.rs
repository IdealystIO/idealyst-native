//! End-to-end: a real `idealyst dev --web --local --events-file` session
//! reports each save as the right sequence of events — in the file and on
//! the page — and the page's own acks come back into the session.
//!
//! Five saves, in order, with a headless Chrome on the page:
//!
//! 1. a literal edit → `decided{overlay}`, `overlay_pushed`, and the
//!    page's `page_ack{overlay}` with the applier's counts;
//! 2. a body edit → `decided{hot_patch}`, `patch_built`, and
//!    `page_ack{hot_patch}` with the functions redirected;
//! 3. a body edit that does not compile → the patch fails, the rebuild
//!    fails, and a `diagnostic` with the file and line reaches the file AND
//!    the page, whose status overlay shows it;
//! 4. the fix → a hot patch again, and the overlay's error panel clears;
//! 5. a shape edit → `decided{rebuild}` with its reason, a rebuild, and the
//!    page acking `reloading` onto the new generation.
//!
//! The page side is observed two ways: a second `EventSource` the test
//! opens in the page records every `dev-state` event the page is sent,
//! and the status overlay's shadow DOM is read for what the user sees.
//!
//! The project: by default a small app materialized here (path-pinned to
//! the in-tree framework). With `IDEALYST_E2E_LAB=<dir>` it is a COPY of
//! that project instead — the hot-reload lab — renamed so it shares no
//! build dirs with a session already running there, with its `[patch]`
//! pointed at this checkout.
//!
//! `#[ignore]`d: it compiles the framework for wasm32 and needs Chrome (or
//! `IDEALYST_BROWSER`) and `wasm-bindgen` on `PATH`:
//!
//! ```text
//! cargo test -p idealyst-cli --test dev_events_e2e -- --ignored --nocapture
//! IDEALYST_E2E_LAB=~/Desktop/hotreload-lab cargo test -p idealyst-cli \
//!     --test dev_events_e2e -- --ignored --nocapture
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

/// One save: the file, and the text replaced in it.
struct Edit {
    file: &'static str,
    from: &'static str,
    to: &'static str,
}

/// The five saves, for a given project.
struct Script {
    /// Text on the page once the app has rendered.
    ready: &'static str,
    literal: Edit,
    body: Edit,
    /// Text the body edit puts on the page.
    body_shows: &'static str,
    broken: Edit,
    fix: Edit,
    fix_shows: &'static str,
    shape: Edit,
}

const APP_RS: &str = r#"
use runtime_core::{component, signal, ui, Element};

#[component]
fn Badge(label: String) -> Element {
    ui! { view { text { "[ {label} ]" } } }
}

fn logic_line(n: i32) -> String {
    format!("logic v1 -> {}", n * 2)
}

#[component]
fn Root() -> Element {
    let count = signal(0i32);
    let bump = move || count.set(count.get() + 1);
    ui! {
        view {
            text { "a literal the overlay tier patches" }
            Badge(label = "draft".to_string())
            button(label = "+1".to_string(), on_click = bump)
            text { move || logic_line(count.get()) }
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
  <head><meta charset="utf-8" /><base href="/" /><title>dev events e2e</title></head>
  <body>
    <div id="app"></div>
    <script type="module">
      import init from "/pkg/dev_events_e2e.js";
      init();
    </script>
  </body>
</html>
"#;

fn materialize(dir: &Path, repo: &Path) -> Script {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let dep = |p: &str| repo.join(p).display().to_string();
    std::fs::write(
        dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "dev-events-e2e"
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

[package.metadata.idealyst.app]
name = "Dev Events E2E"
bundle_id = "com.example.dev_events_e2e"
version = "0.0.1"
targets = ["web"]

[workspace]
"#,
            idealyst = dep("crates/idealyst"),
            core = dep("crates/runtime/core"),
            vocab = dep("crates/runtime/vocabulary"),
            scene = dep("crates/runtime/scene"),
        ),
    )
    .unwrap();
    std::fs::write(dir.join("src/lib.rs"), LIB_RS).unwrap();
    std::fs::write(dir.join("src/app.rs"), APP_RS).unwrap();
    std::fs::write(dir.join("src/main.rs"), "idealyst::entry!(dev_events_e2e);\n").unwrap();
    std::fs::write(dir.join("index.html"), INDEX_HTML).unwrap();
    Script {
        ready: "logic v1 -> 0",
        literal: Edit {
            file: "src/app.rs",
            from: "a literal the overlay tier patches",
            to: "an edited literal",
        },
        body: Edit { file: "src/app.rs", from: "logic v1 ->", to: "logic v2 ->" },
        body_shows: "logic v2 -> 0",
        broken: Edit {
            file: "src/app.rs",
            from: "format!(\"logic v2 -> {}\", n * 2)",
            to: "{ let broken: u32 = \"not a number\"; format!(\"logic v2 -> {} {}\", n * 2, broken) }",
        },
        fix: Edit {
            file: "src/app.rs",
            from: "{ let broken: u32 = \"not a number\"; format!(\"logic v2 -> {} {}\", n * 2, broken) }",
            to: "format!(\"logic v3 -> {}\", n * 2)",
        },
        fix_shows: "logic v3 -> 0",
        shape: Edit {
            file: "src/app.rs",
            from: "fn Badge(label: String) -> Element {",
            to: "fn Badge(label: String, #[prop(default = 7)] n: i32) -> Element {\n    let _ = n;",
        },
    }
}

/// Copy the lab at `src` to `dir`, renamed and patched at this checkout.
fn copy_lab(src: &Path, dir: &Path, repo: &Path) -> Script {
    // `-rlI`, not `-a`: every file is rewritten (`-I`) and gets a fresh
    // modification time (no `-t`). Restoring the source's older mtime over
    // a file the previous run edited makes cargo's freshness check call the
    // previous build current, and the session serves the previous run's
    // edits — which is exactly what the first version of this copy did.
    let status = Command::new("rsync")
        .args(["-rlI", "--delete", "--exclude", "target", "--exclude", "pkg", "--exclude", ".idealyst"])
        .arg(format!("{}/", src.display()))
        .arg(format!("{}/", dir.display()))
        .status()
        .expect("rsync the lab");
    assert!(status.success());
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    let old_name = manifest
        .lines()
        .find_map(|l| l.strip_prefix("name = \""))
        .and_then(|l| l.strip_suffix('"'))
        .expect("the lab's package name")
        .to_string();
    let new_name = format!("{old_name}-events-e2e");
    // Every `[patch]` path points at THIS checkout: the test exercises the
    // framework it was built from, whatever the lab was pointed at.
    let mut in_patch = false;
    let rewritten: Vec<String> = manifest
        .lines()
        .map(|line| {
            if line.starts_with('[') {
                in_patch = line.starts_with("[patch");
            }
            if line.starts_with("name = \"") && !in_patch {
                return line.replace(&old_name, &new_name);
            }
            if in_patch {
                if let (Some(start), Some(at)) = (line.find("path = \""), line.find("/crates/")) {
                    let prefix = &line[..start + "path = \"".len()];
                    return format!("{prefix}{}{}", repo.display(), &line[at..]);
                }
            }
            line.to_string()
        })
        .collect();
    std::fs::write(dir.join("Cargo.toml"), rewritten.join("\n") + "\n").unwrap();
    let (old_lib, new_lib) = (old_name.replace('-', "_"), new_name.replace('-', "_"));
    for f in ["src/main.rs", "index.html"] {
        let text = std::fs::read_to_string(dir.join(f)).unwrap();
        std::fs::write(dir.join(f), text.replace(&old_lib, &new_lib)).unwrap();
    }
    Script {
        ready: "logic v1 -> 0",
        literal: Edit {
            file: "src/app.rs",
            from: "1. Edit this sentence and save.",
            to: "1. Edited by the events e2e.",
        },
        body: Edit { file: "src/app.rs", from: "logic v1 ->", to: "logic v2 ->" },
        body_shows: "logic v2 -> 0",
        broken: Edit {
            file: "src/app.rs",
            from: "format!(\"logic v2 -> {}\", n * 2)",
            to: "{ let broken: u32 = \"not a number\"; format!(\"logic v2 -> {} {}\", n * 2, broken) }",
        },
        fix: Edit {
            file: "src/app.rs",
            from: "{ let broken: u32 = \"not a number\"; format!(\"logic v2 -> {} {}\", n * 2, broken) }",
            to: "format!(\"logic v3 -> {}\", n * 2)",
        },
        fix_shows: "logic v3 -> 0",
        shape: Edit {
            file: "src/app.rs",
            from: "fn Pill(label: String) -> Element {",
            to: "fn Pill(label: String, #[prop(default = 7)] n: i32) -> Element {\n    let _ = n;",
        },
    }
}

fn apply(project: &Path, edit: &Edit) {
    let path = project.join(edit.file);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(edit.from), "{} does not contain {:?}", edit.file, edit.from);
    std::fs::write(&path, text.replacen(edit.from, edit.to, 1)).unwrap();
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

    /// Every event written so far.
    fn events(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.events)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Wait until an event after `after` (a seq) satisfies `pred`.
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

/// The event types after `after`, one per line, without the noisy ones.
fn summary(events: &[Value], after: u64) -> String {
    events
        .iter()
        .filter(|e| e["seq"].as_u64().unwrap_or(0) > after)
        .filter(|e| !matches!(e["type"].as_str(), Some("output" | "cargo_progress" | "log")))
        .map(|e| e.to_string().chars().take(220).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The sequence of typed-event kinds after `after` (tier-bearing ones
/// with their tier), for order assertions.
fn kinds(events: &[Value], after: u64, until: u64) -> Vec<String> {
    events
        .iter()
        .filter(|e| {
            let s = e["seq"].as_u64().unwrap_or(0);
            s > after && s <= until
        })
        .filter_map(|e| {
            let t = e["type"].as_str()?;
            Some(match t {
                "decided" => format!("decided:{}", e["decision"]["tier"].as_str()?),
                "page_ack" => format!("page_ack:{}", e["ack"]["kind"].as_str()?),
                "build_finished" => format!("build_finished:{}", e["outcome"].as_str()?),
                "change_detected" | "overlay_pushed" | "patch_built" | "patch_failed"
                | "build_started" => t.to_string(),
                "diagnostic" if e["diagnostic"]["level"] == "error" => "diagnostic:error".into(),
                _ => return None,
            })
        })
        .collect()
}

/// `needles` appear in `haystack` in this order (other items between).
fn in_order(haystack: &[String], needles: &[&str]) -> bool {
    let mut it = haystack.iter();
    needles.iter().all(|n| it.any(|h| h == n))
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

fn tail(log: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
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

/// A minimal GET that reads exactly `Content-Length` bytes (Chrome's
/// DevTools endpoint keeps the connection open).
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

    /// Record every `dev-state` event this page is sent, on a second
    /// stream beside the reload script's own.
    fn record_dev_state(&mut self) {
        self.eval(
            "(() => { window.__e2e_states = []; \
               const es = new EventSource('/__idealyst/reload'); \
               es.addEventListener('dev-state', e => window.__e2e_states.push(JSON.parse(e.data))); \
               return new Promise(r => es.onopen = () => r(true)); })()",
        );
    }

    fn dev_states(&mut self) -> Vec<Value> {
        self.eval("window.__e2e_states || []").as_array().cloned().unwrap_or_default()
    }

    /// Wait until the page has been sent every one of `kinds` (as
    /// `kinds()` spells them) after sequence number `after`, in order.
    fn wait_dev_states(&mut self, after: u64, want: &[&str], budget: Duration) -> Vec<String> {
        let deadline = Instant::now() + budget;
        loop {
            let got = kinds(&self.dev_states(), after, u64::MAX);
            if in_order(&got, want) {
                return got;
            }
            assert!(
                Instant::now() < deadline,
                "the page was not sent {want:?} after {after}; it got {got:?}"
            );
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// What the status overlay shows: the badge, whether the error panel
    /// is up, and the panel's text.
    fn overlay(&mut self) -> Value {
        self.eval(
            "(() => { const h = document.querySelector('idealyst-dev-overlay'); \
               if (!h) return null; const r = h.shadowRoot || h; \
               const p = r.querySelector('[data-part=panel]'); \
               return { badge: r.querySelector('[data-part=badge]').textContent, \
                        panel: p.style.display !== 'none', panelText: p.innerText || p.textContent }; })()",
        )
    }
}

fn start(project: &Path, scratch: &Path) -> (Session, Page) {
    let chrome =
        browser().expect("this test needs Chrome/Chromium, or IDEALYST_BROWSER pointing at one");
    let log = Arc::new(Mutex::new(String::new()));
    let events = scratch.join("events.jsonl");
    let mut session = Session { children: Vec::new(), log: log.clone(), events: events.clone() };
    let port = free_port();
    let mut dev = Command::new(env!("CARGO_BIN_EXE_idealyst"))
        .current_dir(project)
        // This test drives its own headless Chrome; never open the user's.
        .env("IDEALYST_TEST_DRIVER", "1")
        .args(["dev", "--web", "--local", "--no-robot", "--no-headless-client"])
        .args(["--port", &port.to_string()])
        .arg("--events-file")
        .arg(&events)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn idealyst dev");
    pump(dev.stdout.take().unwrap(), log.clone());
    pump(dev.stderr.take().unwrap(), log.clone());
    session.children.push(dev);

    // A cold framework build for wasm32 is minutes.
    session.wait_event(0, Duration::from_secs(1200), "livereload server", |e| {
        e["type"] == "server_ready" && e["kind"] == "livereload"
    });

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
            .arg(format!("--user-data-dir={}", scratch.join("chrome-profile").display()))
            .arg(format!("http://127.0.0.1:{port}/"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn Chrome"),
    );
    (session, Page::attach(debug_port))
}

#[test]
#[ignore = "compiles the framework for wasm32 and drives headless Chrome; run with --ignored"]
fn every_tier_is_reported_in_the_file_and_on_the_page_and_the_page_acks_it() {
    let repo = repo_root();
    let scratch = tempfile::tempdir().unwrap();
    // A stable directory: the web target dir is keyed by the project path
    // while the hot tier is armed, so a fresh path is a cold build.
    let (project, script) = match std::env::var_os("IDEALYST_E2E_LAB") {
        Some(lab) => {
            let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dev_events_e2e_lab");
            let script = copy_lab(Path::new(&lab), &dir, &repo);
            (dir, script)
        }
        None => {
            let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("dev_events_e2e");
            let script = materialize(&dir, &repo);
            (dir, script)
        }
    };

    let (session, mut page) = start(&project, scratch.path());
    assert!(
        page.wait(&format!("document.body.innerText.includes({:?})", script.ready), Duration::from_secs(90)),
        "the app never rendered: {:?}",
        page.text()
    );

    // The session opened with its line, its servers, and the initial
    // build — typed.
    let opening = session.events();
    assert_eq!(opening[0]["type"], "session_started", "{}", summary(&opening, 0));
    assert_eq!(opening[0]["v"], 1);
    assert_eq!(opening[0]["hot_tier"]["state"], "armed");
    let first_build = kinds(&opening, 0, u64::MAX);
    assert!(
        in_order(&first_build, &["build_started", "build_finished:ready"]),
        "{first_build:?}"
    );
    assert!(
        opening.iter().any(|e| e["type"] == "cargo_progress" && e["total"].as_u64().unwrap_or(0) > 0),
        "the initial build reported cargo progress against a total"
    );
    session.wait_event(0, Duration::from_secs(30), "the page acking its connection", |e| {
        e["type"] == "page_ack" && e["ack"]["kind"] == "connected"
    });
    page.record_dev_state();

    // ── 1. literal → overlay ─────────────────────────────────────────
    let before = session.last_seq();
    apply(&project, &script.literal);
    let ack = session.wait_event(before, Duration::from_secs(60), "the page's overlay ack", |e| {
        e["type"] == "page_ack" && e["ack"]["kind"] == "overlay"
    });
    assert!(ack["ack"]["applied"].is_u64(), "the ack carries the applier's counts: {ack}");
    let k = kinds(&session.events(), before, ack["seq"].as_u64().unwrap());
    assert!(
        in_order(&k, &["change_detected", "decided:overlay", "overlay_pushed", "page_ack:overlay"]),
        "{k:?}"
    );
    // The page's status overlay was fed the same facts.
    page.wait_dev_states(before, &["change_detected", "decided:overlay", "overlay_pushed"], Duration::from_secs(10));

    // ── 2. body → hot patch ─────────────────────────────────────────
    let before = session.last_seq();
    apply(&project, &script.body);
    let ack = session.wait_event(before, Duration::from_secs(180), "the page's hot-patch ack", |e| {
        e["type"] == "page_ack" && e["ack"]["kind"] == "hot_patch"
    });
    assert!(ack["ack"]["redirected"].as_u64().unwrap_or(0) > 0, "{ack}");
    let k = kinds(&session.events(), before, ack["seq"].as_u64().unwrap());
    assert!(
        in_order(&k, &["change_detected", "decided:hot_patch", "patch_built", "page_ack:hot_patch"]),
        "{k:?}"
    );
    page.wait_dev_states(before, &["change_detected", "decided:hot_patch", "patch_built"], Duration::from_secs(10));
    assert!(
        page.wait(&format!("document.body.innerText.includes({:?})", script.body_shows), Duration::from_secs(10)),
        "{:?}",
        page.text()
    );

    // ── 3. a compile error ──────────────────────────────────────────
    let before = session.last_seq();
    apply(&project, &script.broken);
    let failed = session.wait_event(before, Duration::from_secs(600), "the failed build", |e| {
        e["type"] == "build_finished" && e["outcome"] == "failed"
    });
    let k = kinds(&session.events(), before, failed["seq"].as_u64().unwrap());
    assert!(
        in_order(
            &k,
            &["decided:hot_patch", "patch_failed", "build_started", "diagnostic:error", "build_finished:failed"]
        ),
        "{k:?}"
    );
    let diag = session
        .events()
        .into_iter()
        .find(|e| {
            e["seq"].as_u64().unwrap_or(0) > before
                && e["type"] == "diagnostic"
                && e["diagnostic"]["level"] == "error"
        })
        .unwrap();
    let file = diag["diagnostic"]["file"].as_str().expect("the error names its file");
    let line = diag["diagnostic"]["line"].as_u64().expect("and its line");
    assert!(file.ends_with("src/app.rs"), "{diag}");
    // The page was sent it, and shows it.
    assert!(
        page.wait(
            "(window.__e2e_states || []).some(s => s.type === 'diagnostic' && s.diagnostic.file)",
            Duration::from_secs(10)
        ),
        "the diagnostic never reached the page: {:?}",
        page.dev_states()
    );
    let page_diag = page
        .dev_states()
        .into_iter()
        .find(|s| s["type"] == "diagnostic")
        .unwrap();
    assert_eq!(page_diag["diagnostic"]["file"], file);
    assert_eq!(page_diag["diagnostic"]["line"], line);
    let overlay = page.overlay();
    assert_eq!(overlay["panel"], true, "the error panel is up: {overlay}");
    let shown = overlay["panelText"].as_str().unwrap_or_default();
    assert!(shown.contains(&format!("{file}:{line}")), "the panel shows file:line: {shown}");

    // ── 4. the fix clears it ────────────────────────────────────────
    let before = session.last_seq();
    apply(&project, &script.fix);
    session.wait_event(before, Duration::from_secs(600), "the fix to land", |e| {
        e["type"] == "patch_built"
            || (e["type"] == "build_finished" && e["outcome"] != "failed")
    });
    assert!(
        page.wait(
            "(() => { const h = document.querySelector('idealyst-dev-overlay'); \
               const r = h && (h.shadowRoot || h); \
               return !!r && r.querySelector('[data-part=panel]').style.display === 'none'; })()",
            Duration::from_secs(30)
        ),
        "the error panel did not clear after the fix: {}",
        page.overlay()
    );
    assert!(
        page.wait(&format!("document.body.innerText.includes({:?})", script.fix_shows), Duration::from_secs(60)),
        "{:?}",
        page.text()
    );

    // ── 5. shape → rebuild and reload ───────────────────────────────
    let page_states_before_reload = page.dev_states();
    let before = session.last_seq();
    apply(&project, &script.shape);
    let reloading = session.wait_event(before, Duration::from_secs(600), "the page reloading", |e| {
        e["type"] == "page_ack" && e["ack"]["kind"] == "reloading"
    });
    let k = kinds(&session.events(), before, reloading["seq"].as_u64().unwrap());
    assert!(
        in_order(&k, &["decided:rebuild", "build_started", "build_finished:reloaded", "page_ack:reloading"]),
        "{k:?}"
    );
    let reason = session
        .events()
        .into_iter()
        .find(|e| e["seq"].as_u64().unwrap_or(0) > before && e["type"] == "decided")
        .unwrap();
    assert!(reason["decision"]["reason"].is_string(), "a rebuild says why: {reason}");
    let reloaded_gen = reloading["ack"]["gen"].as_u64().unwrap();
    session.wait_event(reloading["seq"].as_u64().unwrap(), Duration::from_secs(60), "the page reconnecting", |e| {
        e["type"] == "page_ack" && e["ack"]["kind"] == "connected" && e["ack"]["gen"] == reloaded_gen
    });
    // The reloaded page never saw the rebuild happen — but a page that
    // connects is sent the session's current state first, so it knows.
    page.record_dev_state();
    page.wait_dev_states(
        before,
        &["decided:rebuild", "build_started", "build_finished:reloaded"],
        Duration::from_secs(10),
    );

    // Everything the page was sent before the reload arrived in the
    // file's order: one stream, two consumers.
    let file_seqs: Vec<u64> = session.events().iter().filter_map(|e| e["seq"].as_u64()).collect();
    let page_seqs: Vec<u64> =
        page_states_before_reload.iter().filter_map(|s| s["seq"].as_u64()).collect();
    assert!(page_seqs.windows(2).all(|w| w[0] < w[1]), "page order: {page_seqs:?}");
    assert!(page_seqs.iter().all(|s| file_seqs.contains(s)), "the page saw events the file did not");

    // `IDEALYST_E2E_EVENTS_OUT=<path>` keeps the session's events — the
    // recording `crates/dev/events/tests/fixtures` validates the schema
    // against comes from here.
    if let Some(out) = std::env::var_os("IDEALYST_E2E_EVENTS_OUT") {
        std::fs::copy(&session.events, out).expect("copy the events file");
    }

    // The plain lines a terminal (and this test's older sibling) reads
    // are still there.
    let log = session.log();
    assert!(log.contains("[dev] patched 1 site(s) in"), "{}", tail(&log));
    assert!(log.contains("function(s) redirected"), "{}", tail(&log));
    assert!(log.contains("[dev] rebuilding: "), "{}", tail(&log));
    assert!(log.contains("[dev-reload] rebuild failed: "), "{}", tail(&log));
}
