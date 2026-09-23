//! End-to-end: a body edit under `idealyst dev --web --local` lands as a
//! wasm hot patch in a RUNNING page, with app state intact and no reload;
//! a shape edit still rebuilds and reloads.
//!
//! Every piece of the tier has unit tests and `build-web`'s
//! `wasm_patch_roundtrip` compiles a real crate through it. Neither can
//! see what actually broke the tier three times on its way to working:
//! a symbol the base knew by a second name, a JS shim with no table
//! slot, and a relocation function the resolver threw away — the last
//! one only visible as "null function" inside the browser, after the
//! patch had applied. So this drives the real thing:
//!
//! 1. Materialize a small app (a counter, a free function whose body is
//!    edited, a `#[component]` whose shape is edited), path-pinned to the
//!    in-tree framework crates.
//! 2. Spawn the actual `idealyst dev --web --local` binary.
//! 3. Open the page in headless Chrome and talk to it over the DevTools
//!    protocol: bump the counter, set a marker on `window`.
//! 4. Edit a function body. Poll until the new text is on screen, then
//!    assert the counter kept its value and the marker is still there —
//!    a reload would have cleared both — and that a `thread_local`
//!    signal cache still reads (the rebuild keeps the page's world).
//! 5. Add a prop to the component. Poll until the page shows it, and
//!    assert the marker is GONE: a shape change must rebuild and reload.
//!
//! `#[ignore]`d: it compiles the framework for wasm32 and needs Chrome
//! (or `IDEALYST_BROWSER`) and `wasm-bindgen` on `PATH`. Run it
//! deliberately:
//!
//! ```text
//! cargo test -p idealyst-cli --test wasm_hot_patch_e2e -- --ignored --nocapture
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

const APP_RS: &str = r#"
use std::cell::RefCell;

use runtime_core::{component, signal, ui, Element, Signal};

thread_local! {
    // A world-lifetime cache, the shape real apps have (CrewForge keeps
    // several). A rebuild that replaced the world left this pointing into
    // a dead one, and the first render after a patch panicked.
    static VISITS: RefCell<Option<Signal<i32>>> = const { RefCell::new(None) };
}

fn visits() -> Signal<i32> {
    VISITS.with(|c| *c.borrow_mut().get_or_insert_with(|| runtime_core::unscope(|| signal(41))))
}

#[component]
fn Badge(label: String) -> Element {
    ui! { view { text { "[ {label} ]" } } }
}

// The body this test edits. A plain function: it reaches the screen
// only because `Root` — redirected through the jump table — calls the
// patch's copy of it.
fn logic_line(n: i32) -> String {
    format!("logic v1 -> {}", n * 2)
}

#[component]
fn Root() -> Element {
    let count = signal(0i32);
    let bump = move || count.set(count.get() + 1);
    ui! {
        view {
            Badge(label = "draft".to_string())
            text { move || format!("count = {}", count.get()) }
            button(label = "+1".to_string(), on_click = bump)
            text { move || logic_line(count.get()) }
            text { move || format!("visits = {}", visits().get()) }
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
  <head><meta charset="utf-8" /><base href="/" /><title>hot patch e2e</title></head>
  <body>
    <div id="app"></div>
    <script type="module">
      import init from "/pkg/hotpatch_e2e.js";
      init();
    </script>
  </body>
</html>
"#;

fn materialize(dir: &Path, repo: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let dep = |p: &str| repo.join(p).display().to_string();
    std::fs::write(
        dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "hotpatch-e2e"
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
name = "Hot Patch E2E"
bundle_id = "com.example.hotpatch_e2e"
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
    std::fs::write(dir.join("src/main.rs"), "idealyst::entry!(hotpatch_e2e);\n").unwrap();
    std::fs::write(dir.join("index.html"), INDEX_HTML).unwrap();
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Kills its children on drop, so a failed assertion does not leave a
/// dev server or a browser running.
struct Session {
    children: Vec<Child>,
    log: Arc<Mutex<String>>,
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

    fn wait_for_log(&self, needle: &str, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            if self.log().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        false
    }
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

/// A minimal GET. Chrome's DevTools HTTP endpoint keeps the connection
/// open after the response whatever the request says, so reading to EOF
/// hangs forever; read exactly `Content-Length` bytes instead, under a
/// timeout.
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

/// A DevTools-protocol connection to one page.
struct Page {
    ws: tungstenite::WebSocket<TcpStream>,
    next_id: u64,
}

impl Page {
    fn attach(debug_port: u16) -> Page {
        let deadline = Instant::now() + Duration::from_secs(30);
        let ws_url = loop {
            // Pick the PAGE target. Chrome lists its own UI surfaces too
            // (the omnibox popup is `"type": "browser_ui"` with a
            // `/devtools/page/` URL), and evaluating in one of those
            // never sees the app.
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

    /// Evaluate `expr` in the page and return its JSON value.
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

    fn text(&mut self) -> String {
        self.eval("document.body ? document.body.innerText : ''")
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn wait_for_text(&mut self, needle: &str, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            if self.text().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }
}

#[test]
#[ignore = "compiles the framework for wasm32 and drives headless Chrome; run with --ignored"]
fn a_body_edit_patches_the_running_page_and_a_shape_edit_reloads_it() {
    let repo = repo_root();
    let Some(chrome) = browser() else {
        panic!("this test needs Chrome/Chromium, or IDEALYST_BROWSER pointing at one");
    };
    // A stable directory, not a fresh temp one: an armed hot-patch session
    // keys its web target dir by the project path, so a new path every run
    // is a cold framework build every run. The sources are rewritten below,
    // so a previous run's edits never leak in.
    let tmp = tempfile::tempdir().unwrap();
    let project = Path::new(env!("CARGO_TARGET_TMPDIR")).join("wasm_hot_patch_e2e");
    materialize(&project, &repo);

    let log = Arc::new(Mutex::new(String::new()));
    let mut session = Session { children: Vec::new(), log: log.clone() };

    let port = free_port();
    let mut dev = Command::new(env!("CARGO_BIN_EXE_idealyst"))
        .current_dir(&project)
        .args(["dev", "--web", "--local", "--port", &port.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn idealyst dev");
    pump(dev.stdout.take().unwrap(), log.clone());
    pump(dev.stderr.take().unwrap(), log.clone());
    session.children.push(dev);

    // A cold framework build for wasm32 is minutes, not seconds.
    assert!(
        session.wait_for_log("livereload HTTP at", Duration::from_secs(900)),
        "the dev session never started serving"
    );

    let debug_port = free_port();
    let profile = tmp.path().join("chrome-profile");
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
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg(format!("http://127.0.0.1:{port}/"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn Chrome"),
    );

    let mut page = Page::attach(debug_port);
    assert!(
        page.wait_for_text("logic v1 -> 0", Duration::from_secs(60)),
        "the app never rendered: {:?}",
        page.text()
    );
    assert_eq!(
        page.eval("typeof window.__idealyst_hot_patch"),
        json!("function"),
        "the page did not publish its hot-patch entry point — was the bundle built with \
         `hot-reload`?"
    );

    // State to lose, and a marker only a reload clears.
    page.eval(
        "(async () => { \
            const b = [...document.querySelectorAll('button,[role=button]')] \
                .find(e => e.textContent.trim() === '+1'); \
            for (let i = 0; i < 3; i++) { b.click(); await new Promise(r => setTimeout(r, 50)); } \
            window.__e2e_marker = 'still-here'; \
        })()",
    );
    assert!(page.wait_for_text("count = 3", Duration::from_secs(5)), "{:?}", page.text());

    // ── the body edit ──────────────────────────────────────────────────
    let app_rs = project.join("src/app.rs");
    let source = std::fs::read_to_string(&app_rs).unwrap();
    std::fs::write(&app_rs, source.replace("logic v1 ->", "logic v2 ->")).unwrap();

    assert!(
        page.wait_for_text("logic v2 -> 6", Duration::from_secs(120)),
        "the body edit never reached the page. Page: {:?}\nLog tail:\n{}",
        page.text(),
        tail(&session.log()),
    );
    let log_now = session.log();
    assert!(
        log_now.contains("[hotpatch] src/app.rs ·"),
        "the new text arrived, but not through a hot patch:\n{}",
        tail(&log_now)
    );
    assert_eq!(
        page.eval("window.__e2e_marker"),
        json!("still-here"),
        "the page reloaded — a hot patch must not"
    );
    assert!(
        page.text().contains("count = 3"),
        "the counter lost its value across the patch: {:?}",
        page.text()
    );
    assert!(
        page.text().contains("visits = 41"),
        "a thread_local signal cache did not survive the rebuild: {:?}",
        page.text()
    );

    // The patched page still works: the next click runs the patch's code.
    page.eval(
        "[...document.querySelectorAll('button,[role=button]')] \
            .find(e => e.textContent.trim() === '+1').click()",
    );
    assert!(
        page.wait_for_text("logic v2 -> 8", Duration::from_secs(5)),
        "{:?}",
        page.text()
    );

    // ── the shape edit ─────────────────────────────────────────────────
    let source = std::fs::read_to_string(&app_rs).unwrap();
    std::fs::write(
        &app_rs,
        source
            .replace(
                "fn Badge(label: String) -> Element {",
                "fn Badge(label: String, #[prop(default = 7)] n: i32) -> Element {",
            )
            .replace("text { \"[ {label} ]\" }", "text { \"[ {label} ] {n}\" }"),
    )
    .unwrap();

    assert!(
        page.wait_for_text("[ draft ] 7", Duration::from_secs(300)),
        "the shape edit never reached the page. Page: {:?}\nLog tail:\n{}",
        page.text(),
        tail(&session.log()),
    );
    assert!(
        session.log().contains("changed outside its function bodies"),
        "a shape edit has to be routed to a rebuild:\n{}",
        tail(&session.log())
    );
    assert_eq!(
        page.eval("window.__e2e_marker ?? null"),
        Value::Null,
        "a shape edit has to reload the page; the marker survived, so it was patched instead"
    );
}

fn tail(log: &str) -> String {
    let lines: Vec<&str> = log.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}
