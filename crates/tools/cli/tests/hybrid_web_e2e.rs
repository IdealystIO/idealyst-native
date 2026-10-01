//! End-to-end: an app that still links wasm-bindgen builds in HYBRID mode
//! and runs in a real browser with both namespaces in one instance.
//!
//! Own mode is the default (`build_web::own_glue`): a framework-only app
//! links no wasm-bindgen and the build never runs its CLI. Hybrid is the
//! supported path for everything else, and the one real dependency that
//! still forces it is wgpu — here through `canvas-vello`, the GPU canvas
//! renderer (docs/proposals/own-web-bindings.md, phase 5). So this
//! materializes a small app on the in-tree framework that registers
//! `canvas-native` and then `canvas-vello` (last registration wins), builds
//! it with the real `idealyst build --web`, serves `dist/web`, and drives
//! it in headless Chrome over the DevTools protocol:
//!
//! 1. The build decided hybrid from the linked module (`hybrid mode: … __wbindgen…`),
//!    ran the wasm-bindgen CLI, and staged + fingerprinted the glue file
//!    beside wasm-bindgen's output.
//! 2. The page boots; `globalThis.__idealystGlue` is published; a click
//!    reaches Rust through a web-glue listener whose callbacks, strings and
//!    allocations go through wasm-bindgen's instance — three clicks, three
//!    renders.
//! 3. The canvas. With `navigator.gpu` present, canvas-vello mounts it and
//!    logs which renderer engaged (WebGPU, or the Canvas2D fallback when no
//!    adapter comes back — headless Chrome usually has none). A second load
//!    forces the fallback (`window.__IDEALYST_FORCE_CANVAS2D`, debug builds
//!    only): canvas-vello's web-sys `HtmlCanvasElement` crosses the
//!    `HYBRID-BRIDGE: wgpu` seam into canvas-native's glue rasterizer, and
//!    the pixel read back from the 2D context is the red the scene filled.
//!    Without `navigator.gpu` canvas-vello installs nothing and canvas-native
//!    draws the same red directly; the test says which path it saw.
//!
//! No `console.error` and no uncaught error anywhere.
//!
//! `#[ignore]`d: it compiles the framework and wgpu for wasm32, needs Chrome
//! (or `IDEALYST_BROWSER`) and `wasm-bindgen` on `PATH` at the version the
//! repo's `Cargo.lock` pins (hybrid builds still run its CLI):
//!
//! ```text
//! cargo test -p idealyst-cli --test hybrid_web_e2e -- --ignored --nocapture
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("crates/tools/cli has a workspace root 3 levels up")
        .to_path_buf()
}

const LIB_RS: &str = r#"
use std::rc::Rc;

use canvas::prelude::*;
use runtime_core::{component, signal, ui, view, Element, IntoElement, Length, StyleRules, StyleSheet};

/// canvas-native first, canvas-vello second: the scene registry is
/// `TypeId`-keyed with last registration winning, and canvas-vello only
/// registers where `navigator.gpu` exists.
pub fn register_scene_extensions<H>(registry: &mut runtime_scene::Registry<H>)
where
    H: runtime_vocabulary::caps::ExternalOps
        + runtime_vocabulary::caps::GraphicsOps
        + runtime_vocabulary::style_attach::StyleServices
        + 'static,
{
    canvas_native::register(registry);
    canvas_vello::register(registry);
}

pub fn scene_app() -> Element {
    app()
}

/// A 160×80 canvas filled red, in a box of that size.
fn board() -> Element {
    let fill = StyleRules {
        width: Some(Length::pct(100.0).into()),
        height: Some(Length::pct(100.0).into()),
        ..Default::default()
    };
    let size = StyleRules {
        width: Some(Length::Px(160.0).into()),
        height: Some(Length::Px(80.0).into()),
        ..Default::default()
    };
    let paint = |s: &mut Scene| {
        s.path().add_path(Path::rect(0.0, 0.0, 160.0, 80.0));
        s.fill(Color::new(255, 0, 0, 255));
    };
    let canvas = canvas::Canvas(CanvasProps { draw: canvas::draw(paint), ..Default::default() })
        .with_style(Rc::new(StyleSheet::r#static(fill)))
        .into_element();
    view(vec![canvas]).with_style(Rc::new(StyleSheet::r#static(size))).into_element()
}

#[component]
fn Root() -> Element {
    let count = signal(0i32);
    let bump = move || count.set(count.get() + 1);
    let picture = board();
    ui! {
        view {
            text { "hybrid ready" }
            text { move || format!("count = {}", count.get()) }
            button(label = "+1".to_string(), on_click = bump)
            picture
        }
    }
}

pub fn app() -> Element {
    ui! { Root() }
}
"#;

/// Records console output and uncaught errors before the bundle loads, and
/// arms canvas-vello's debug-only Canvas2D override for `?canvas2d`.
const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" /><base href="/" /><title>hybrid e2e</title>
    <script>
      window.__logs = [];
      window.__errors = [];
      for (const level of ["log", "info", "warn", "error"]) {
        const orig = console[level].bind(console);
        console[level] = (...a) => { window.__logs.push(level + ": " + a.map(String).join(" ")); if (level === "error") window.__errors.push(a.map(String).join(" ")); orig(...a); };
      }
      addEventListener("error", (e) => window.__errors.push(String(e.error && e.error.stack || e.message)));
      addEventListener("unhandledrejection", (e) => window.__errors.push("unhandled: " + String(e.reason && e.reason.stack || e.reason)));
      if (location.search.includes("canvas2d")) window.__IDEALYST_FORCE_CANVAS2D = true;
    </script>
  </head>
  <body>
    <div id="app"></div>
    <script type="module">
      import init from "/pkg/hybrid_e2e.js";
      init();
    </script>
  </body>
</html>
"#;

fn materialize(dir: &Path, repo: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    // The versions the framework is tested with — and the wasm-bindgen the
    // CLI on PATH must match (see `wasm_hot_patch_e2e::seed_lockfile`).
    std::fs::copy(repo.join("Cargo.lock"), dir.join("Cargo.lock")).unwrap();
    let dep = |p: &str| repo.join(p).display().to_string();
    std::fs::write(
        dir.join("Cargo.toml"),
        format!(
            r#"[package]
name = "hybrid-e2e"
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
canvas = {{ path = "{canvas}" }}
canvas-native = {{ path = "{native}" }}
canvas-vello = {{ path = "{vello}" }}

[package.metadata.idealyst.app]
name = "Hybrid E2E"
bundle_id = "com.example.hybrid_e2e"
version = "0.0.1"
targets = ["web"]

[workspace]
"#,
            idealyst = dep("crates/idealyst"),
            core = dep("crates/runtime/core"),
            vocab = dep("crates/runtime/vocabulary"),
            scene = dep("crates/runtime/scene"),
            canvas = dep("crates/sdk/client/canvas"),
            native = dep("crates/sdk/client/canvas/native"),
            vello = dep("crates/sdk/client/canvas/vello"),
        ),
    )
    .unwrap();
    std::fs::write(dir.join("src/lib.rs"), LIB_RS).unwrap();
    std::fs::write(dir.join("src/main.rs"), "idealyst::entry!(hybrid_e2e);\n").unwrap();
    std::fs::write(dir.join("index.html"), INDEX_HTML).unwrap();
}

// ---- static file server -----------------------------------------------------

/// Serves `root` on a free port until the test process exits; an unknown
/// extension-less path is the SPA's `index.html`.
fn serve(root: PathBuf) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let root = root.clone();
            std::thread::spawn(move || {
                let _ = handle(&root, stream);
            });
        }
    });
    port
}

fn handle(root: &Path, mut stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim_end().is_empty() {
            break;
        }
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let path = path.split('?').next().unwrap_or("/");
    let mut rel = path.trim_start_matches('/').to_string();
    if rel.is_empty() || (!rel.contains('.') && !root.join(&rel).is_file()) {
        rel = "index.html".into();
    }
    let file = root.join(&rel);
    let (status, body) = match std::fs::read(&file) {
        Ok(b) if !rel.contains("..") => ("200 OK", b),
        _ => ("404 Not Found", b"not found".to_vec()),
    };
    let ty = match file.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)
}

// ---- headless Chrome over the DevTools protocol -----------------------------

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

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A minimal GET against Chrome's DevTools HTTP endpoint (it keeps the
/// connection open, so read exactly `Content-Length`).
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

struct Browser {
    child: Child,
    page: tungstenite::WebSocket<TcpStream>,
    next_id: u64,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Browser {
    fn open(url: &str, profile: &Path) -> Browser {
        let chrome = browser().expect("this test needs Chrome/Chromium, or IDEALYST_BROWSER pointing at one");
        let debug_port = free_port();
        let child = Command::new(chrome)
            .args([
                "--headless=new",
                "--no-sandbox",
                "--disable-dev-shm-usage",
                "--no-first-run",
                "--no-default-browser-check",
                // Lets headless Chrome offer WebGPU where the machine can;
                // the test passes either way and reports which path ran.
                "--enable-unsafe-webgpu",
            ])
            .arg(format!("--remote-debugging-port={debug_port}"))
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn Chrome");
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
        let (page, _) = tungstenite::client(ws_url.as_str(), stream).expect("CDP handshake");
        Browser { child, page, next_id: 1 }
    }

    fn eval(&mut self, expr: &str) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "id": id,
            "method": "Runtime.evaluate",
            "params": { "expression": expr, "returnByValue": true, "awaitPromise": true },
        });
        self.page.send(tungstenite::Message::text(msg.to_string())).unwrap();
        loop {
            let tungstenite::Message::Text(text) = self.page.read().expect("CDP read") else { continue };
            let v: Value = serde_json::from_str(&text).unwrap();
            if v["id"].as_u64() == Some(id) {
                return v["result"]["result"]["value"].clone();
            }
        }
    }

    fn wait(&mut self, what: &str, expr: &str, budget: Duration) {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            if self.eval(expr) == Value::Bool(true) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "timed out waiting for {what}.\npage: {}\nlogs: {}\nerrors: {}",
            self.eval("document.body ? document.body.innerText : ''"),
            self.eval("JSON.stringify(window.__logs)"),
            self.eval("JSON.stringify(window.__errors)"),
        );
    }

    fn errors(&mut self) -> Value {
        self.eval("window.__errors")
    }
}

/// The marker canvas-vello logs once a canvas picked its renderer.
const MARKER: &str = "window.__logs.some(l => l.includes('canvas-vello: web GPU') || l.includes('canvas-vello: Canvas2D forced'))";

/// The RGBA at the canvas's centre, read through a 2D context — only
/// meaningful on a canvas the Canvas2D path owns (a WebGPU canvas has no
/// 2D context, and `getContext('2d')` then returns null).
const CENTRE_PIXEL: &str = "(() => { const c = document.querySelector('canvas'); const g = c && c.getContext('2d'); \
    if (!g) return null; const d = g.getImageData(c.width >> 1, c.height >> 1, 1, 1).data; return [...d]; })()";

#[test]
#[ignore = "compiles the framework and wgpu for wasm32 and drives headless Chrome; run with --ignored"]
fn a_wgpu_app_builds_hybrid_and_runs_both_namespaces_in_chrome() {
    let repo = repo_root();
    // Stable, so a re-run is an incremental build.
    let project = Path::new(env!("CARGO_TARGET_TMPDIR")).join("hybrid_web_e2e");
    materialize(&project, &repo);

    // ── 1. the build decided hybrid and ran wasm-bindgen ─────────────────
    let out = Command::new(env!("CARGO_BIN_EXE_idealyst"))
        .current_dir(&project)
        .args(["build", "--web"])
        .output()
        .expect("spawn idealyst build");
    let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let tail: String = log.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    assert!(out.status.success(), "idealyst build --web failed:\n{tail}");
    assert!(
        log.contains("hybrid mode: ") && log.contains("__wbindgen"),
        "the build did not decide hybrid from wasm-bindgen's imports:\n{tail}"
    );
    let timing = log.lines().find(|l| l.contains("timing: total")).unwrap_or_default().to_string();
    for stage in ["wasm-bindgen ", "glue-extract "] {
        assert!(timing.contains(stage), "no {stage}stage: {timing}");
    }
    assert!(!timing.contains("glue-package "), "own-mode packaging ran too: {timing}");
    let pkg = project.join("dist/web/pkg");
    let staged: Vec<String> = std::fs::read_dir(&pkg)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        staged.iter().any(|f| f.starts_with("__idealyst_glue.") && f.ends_with(".js")),
        "the glue file was not staged and fingerprinted: {staged:?}"
    );

    let port = serve(project.join("dist/web"));
    let scratch = tempfile::tempdir().unwrap();

    // ── 2. both namespaces in one instance ───────────────────────────────
    let mut page = Browser::open(&format!("http://127.0.0.1:{port}/"), &scratch.path().join("a"));
    page.wait("boot", "document.body.innerText.includes('hybrid ready')", Duration::from_secs(60));
    assert_eq!(page.eval("typeof globalThis.__idealystGlue"), json!("object"));
    for _ in 0..3 {
        page.eval(
            "[...document.querySelectorAll('button,[role=button]')].find(e => e.textContent.trim() === '+1').click()",
        );
    }
    page.wait("three glue-listener clicks", "document.body.innerText.includes('count = 3')", Duration::from_secs(10));

    // ── 3. the canvas ────────────────────────────────────────────────────
    let gpu = page.eval("!!navigator.gpu") == Value::Bool(true);
    page.wait("a canvas", "!!document.querySelector('canvas')", Duration::from_secs(10));
    if gpu {
        page.wait("canvas-vello's renderer decision", MARKER, Duration::from_secs(30));
        let which = page.eval("window.__logs.find(l => l.includes('canvas-vello: web GPU'))");
        eprintln!("[hybrid e2e] navigator.gpu present; canvas-vello: {which}");
    } else {
        // canvas-vello's gate left canvas-native's handler in place.
        page.wait("canvas-native's paint", &format!("JSON.stringify({CENTRE_PIXEL}) === '[255,0,0,255]'"), Duration::from_secs(10));
        eprintln!("[hybrid e2e] no navigator.gpu: canvas-native drew the canvas; the bridge was not exercised");
    }
    assert_eq!(page.errors(), json!([]), "errors on the page");
    drop(page);

    // The HYBRID-BRIDGE: canvas-vello hands its web-sys canvas to
    // canvas-native's glue rasterizer.
    if gpu {
        let mut page = Browser::open(&format!("http://127.0.0.1:{port}/?canvas2d"), &scratch.path().join("b"));
        page.wait("boot", "document.body.innerText.includes('hybrid ready')", Duration::from_secs(60));
        page.wait(
            "the forced Canvas2D fallback",
            "window.__logs.some(l => l.includes('canvas-vello: Canvas2D forced'))",
            Duration::from_secs(30),
        );
        page.wait(
            "the red the scene filled, through the bridged canvas",
            &format!("JSON.stringify({CENTRE_PIXEL}) === '[255,0,0,255]'"),
            Duration::from_secs(10),
        );
        eprintln!("[hybrid e2e] forced Canvas2D: canvas-vello → HYBRID-BRIDGE → canvas-native painted red");
        assert_eq!(page.errors(), json!([]), "errors on the page");
    }
}
