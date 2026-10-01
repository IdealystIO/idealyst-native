//! Phase 1 of `docs/proposals/own-web-bindings.md`, end to end: the
//! framework-owned JS boundary (`web-glue` + `build_web::own_glue`) in a
//! real browser.
//!
//! * `own_glue_demo_runs_in_chrome_without_wasm_bindgen` — builds
//!   `tests/own-glue/demo` (web-glue only) through the own-mode pass and
//!   asserts DOM creation, attributes/text, an event listener mutating Rust
//!   state, an awaited timer Promise and a rejection, a caught JS exception,
//!   the in-page self-checks (non-ASCII strings, memory growth during a JS
//!   → Rust string, handle-slab accounting, listener detach, stale-callback
//!   refusal, the custom-section JS module), that a benchmark's 10 000 retained
//!   row handles are all released afterwards, and that static constructors
//!   ran once, and a `web_glue::worker` Worker running a Rust fn in a fresh
//!   instance of the module. Also asserts the crate graph and
//!   the output carry no trace of wasm-bindgen.
//! * `hybrid_module_runs_with_both_namespaces` — `tests/own-glue/hybrid`,
//!   which uses web-glue AND wasm-bindgen, packaged in hybrid mode: both
//!   namespaces supplied, glue callbacks/strings reach web-glue's exports
//!   through wasm-bindgen's instance, and static constructors ran once.
//! * `measure_own_glue_against_web_sys` — the decision-gate numbers (build
//!   time, output size, call overhead vs the web-sys twin). Prints a table;
//!   asserts only that both variants ran the workload.
//!
//! All `#[ignore]`d: they compile crates for wasm32 and drive headless
//! Chrome (or `IDEALYST_BROWSER`). The hybrid and measurement tests also
//! need `wasm-bindgen` on PATH (matching the locked crate, 0.2.128). Run
//! them one at a time — they share a target dir:
//!
//! ```text
//! cargo test -p build-web --test own_glue_e2e -- --ignored --nocapture --test-threads=1
//! ```
//!
//! The DevTools-protocol client below mirrors
//! `crates/tools/cli/tests/wasm_hot_patch_e2e.rs` (integration tests cannot
//! share code across crates without a helper crate; the two copies are
//! ~80 lines).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use build_web::own_glue::{build_crate, CrateBuild, CrateBuildReport, Mode};
use serde_json::{json, Value};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("crates/tools/build/web has a workspace root 4 levels up")
        .to_path_buf()
}

/// Shared across runs so a re-run is incremental; separate per mode because
/// own mode's RUSTFLAGS change every dependency's fingerprint.
fn target_dir(mode: &str) -> PathBuf {
    std::env::var_os("OWN_GLUE_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("target").join("own-glue-e2e"))
        .join(mode)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("own-glue-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn build(package: &str, mode: Mode, release: bool, extra: &[&str], site: &Path) -> CrateBuildReport {
    let mode_dir = match mode {
        Mode::Own => "own",
        Mode::Hybrid => "bindgen",
    };
    build_crate(
        &dev_events::Reporter::default(),
        &CrateBuild {
            manifest_dir: repo_root(),
            package: package.into(),
            bin: package.into(),
            release,
            mode,
            target_dir: target_dir(mode_dir),
            out_dir: site.join("pkg"),
            extra_cargo_args: extra.iter().map(|s| s.to_string()).collect(),
        },
    )
    .unwrap_or_else(|e| panic!("build {package}: {e:#}"))
}

fn index_html(dir: &Path, script: &str) {
    // Failures surface through the listeners: a module script cannot wrap
    // its `import`s in try/catch, and a top-level-await rejection is
    // reported as an `error` event.
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>own-glue</title>\n\
         <script>window.__errors = [];\n\
         addEventListener('error', (e) => __errors.push(String(e.error && e.error.stack || e.message)));\n\
         addEventListener('unhandledrejection', (e) => __errors.push('unhandled: ' + String(e.reason && e.reason.stack || e.reason)));</script>\n\
         <script type=module>\n{script}\nwindow.__booted = true;\n</script>\n",
    );
    std::fs::write(dir.join("index.html"), html).unwrap();
}

// ---- static file server -----------------------------------------------------

/// Serves `root` on a free port until the test process exits. One thread,
/// one connection at a time, `Connection: close` — enough for a page, its
/// JS and its wasm.
fn serve(root: PathBuf) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = handle(&root, stream);
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
    let rel = if path == "/" { "index.html" } else { path.trim_start_matches('/') };
    let file = root.join(rel);
    let (status, body) = match std::fs::read(&file) {
        Ok(b) if !rel.contains("..") => ("200 OK", b),
        _ => ("404 Not Found", b"not found".to_vec()),
    };
    let ty = match file.extension().and_then(|e| e.to_str()) {
        Some("wasm") => "application/wasm",
        Some("js") => "text/javascript",
        Some("html") => "text/html; charset=utf-8",
        _ => "application/octet-stream",
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {ty}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
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

/// Chrome's DevTools HTTP endpoint keeps the connection open after the
/// response, so read exactly `Content-Length` bytes.
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
    chrome: Child,
    _profile: PathBuf,
    ws: tungstenite::WebSocket<TcpStream>,
    next_id: u64,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.chrome.kill();
        let _ = self.chrome.wait();
    }
}

impl Browser {
    fn open(url: &str, profile: PathBuf) -> Browser {
        let chrome = browser().expect("this test needs Chrome/Chromium, or IDEALYST_BROWSER");
        let debug_port = free_port();
        let chrome = Command::new(chrome)
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
                        let is_app = t["type"] == "page"
                            && t["url"].as_str().is_some_and(|u| u.starts_with("http"));
                        is_app.then(|| t["webSocketDebuggerUrl"].as_str().map(str::to_string))?
                    })
                });
            if let Some(url) = page {
                break url;
            }
            assert!(Instant::now() < deadline, "Chrome never exposed the page target");
            std::thread::sleep(Duration::from_millis(200));
        };
        let stream = TcpStream::connect(("127.0.0.1", debug_port)).unwrap();
        let (ws, _) = tungstenite::client(ws_url.as_str(), stream).expect("CDP handshake");
        Browser { chrome, _profile: profile, ws, next_id: 1 }
    }

    /// Evaluate `expr` (awaiting a promise) and return its JSON value. A
    /// thrown exception is returned as `{"__threw": description}`.
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
                if let Some(ex) = v["result"].get("exceptionDetails") {
                    return json!({ "__threw": ex["exception"]["description"].clone() });
                }
                return v["result"]["result"]["value"].clone();
            }
        }
    }

    fn text(&mut self, id: &str) -> String {
        self.eval(&format!("document.getElementById({id:?})?.textContent ?? '<missing #{id}>'"))
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    fn wait_until(&mut self, what: &str, expr: &str, budget: Duration) {
        let deadline = Instant::now() + budget;
        loop {
            if self.eval(expr) == Value::Bool(true) {
                return;
            }
            let errors = self.eval("window.__errors ?? []");
            assert!(
                errors.as_array().is_some_and(Vec::is_empty),
                "page reported errors while waiting for {what}: {errors}"
            );
            if Instant::now() >= deadline {
                let state = self.eval(
                    "JSON.stringify({ url: location.href, booted: window.__booted ?? null, errors: window.__errors ?? null, body: document.body ? document.body.innerText.slice(0, 2000) : null })",
                );
                panic!("timed out waiting for {what} ({expr}); page state: {state}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn open_site(site: &Path, name: &str) -> Browser {
    let port = serve(site.to_path_buf());
    Browser::open(&format!("http://127.0.0.1:{port}/"), scratch(&format!("{name}-chrome")))
}

// ---- the tests --------------------------------------------------------------

#[test]
#[ignore = "compiles a crate for wasm32 and drives headless Chrome; run with --ignored"]
fn own_glue_demo_runs_in_chrome_without_wasm_bindgen() {
    // The crate graph: no wasm-bindgen, web-sys or js-sys at all.
    let tree = Command::new("cargo")
        .current_dir(repo_root())
        .args(["tree", "-p", "own-glue-demo", "--target", "wasm32-unknown-unknown", "-e", "normal"])
        .output()
        .expect("cargo tree");
    let tree = String::from_utf8_lossy(&tree.stdout);
    for banned in ["wasm-bindgen", "web-sys", "js-sys"] {
        assert!(!tree.contains(banned), "own-glue-demo depends on {banned}:\n{tree}");
    }

    let site = scratch("own-site");
    let report = build("own-glue-demo", Mode::Own, true, &[], &site);
    assert!(report.package.bindgen_time.is_none());
    let js = std::fs::read_to_string(site.join("pkg/own_glue_demo.js")).unwrap();
    let wasm = std::fs::read(site.join("pkg/own_glue_demo_bg.wasm")).unwrap();
    for (what, hay) in [("js", js.as_bytes()), ("wasm", wasm.as_slice())] {
        assert!(
            !hay.windows(6).any(|w| w == b"wbindg" || w == b"__wbg_"),
            "{what} output mentions wasm-bindgen"
        );
    }
    let customs: Vec<String> = wasmparser::Parser::new(0)
        .parse_all(&wasm)
        .filter_map(|p| match p.unwrap() {
            wasmparser::Payload::CustomSection(c) => Some(c.name().to_string()),
            _ => None,
        })
        .collect();
    assert!(!customs.iter().any(|c| c == "__idealyst_glue"), "glue section stripped: {customs:?}");
    assert_eq!(report.package.glue_modules, 1, "the demo's rows.js module was linked");

    index_html(&site, "import init from './pkg/own_glue_demo.js';\n window.__wasm = await init();");
    let mut page = open_site(&site, "own");
    page.wait_until("boot + async chain", "document.getElementById('status')?.textContent === 'ready'", Duration::from_secs(30));

    assert_eq!(page.text("title"), "own-glue demo ✓");
    assert_eq!(page.text("exception"), "caught RangeError: bad value ✓");
    assert_eq!(page.text("async"), "timer 30ms; rejection nope ✓");
    let selftest = page.text("selftest");
    for check in ["strings", "growth", "handles", "callbacks", "module", "reflect", "casts", "listener"] {
        assert!(selftest.contains(&format!("{check}=ok")), "self-check {check} failed: {selftest}");
    }
    // `web_glue::worker` in own mode: a Worker re-imported pkg/<lib>.js,
    // instantiated the same module (its own constructors ran once there)
    // and ran the entry fn by its table index.
    assert_eq!(page.text("worker"), "worker=ok in_worker=true window=false ctors=1 ✓");

    // Events: JS click → Rust closure → Rust state → DOM.
    for _ in 0..3 {
        page.eval("document.getElementById('btn').click()");
    }
    assert_eq!(page.text("count"), "3 (click)");

    // A callback whose Rust owner dropped is a loud JS error.
    let stale = page.eval("(() => { try { window.__staleCallback(); return 'no throw'; } catch (e) { return e.message; } })()");
    assert!(
        stale.as_str().unwrap_or_default().contains("called after its Rust owner dropped it"),
        "{stale}"
    );

    // Idempotent re-init returns the same raw exports (the split loaders
    // rely on `initSync()` after boot).
    let same = page.eval(
        "import('./pkg/own_glue_demo.js').then(async m => m.initSync(undefined, undefined) === window.__wasm && (await m.default()) === window.__wasm)",
    );
    assert_eq!(same, Value::Bool(true), "initSync/init after boot return the raw exports");

    // Handles: a 10k-row benchmark round leaves both sides of the slab
    // where it started.
    let before = page.eval("[__wasm.live_handles(), __wasm.js_live_handles()]");
    page.eval("__wasm.bench_create(10000); __wasm.bench_update(); true");
    assert_eq!(page.eval("document.querySelectorAll('#bench .row').length"), json!(10000));
    assert_eq!(page.eval("document.querySelector('#bench .row:last-child').textContent"), json!("updated 9999"));
    page.eval("__wasm.bench_clear(); true");
    let after = page.eval("[__wasm.live_handles(), __wasm.js_live_handles()]");
    assert_eq!(before, after, "benchmark handles all released");

    // Reactor linkage: constructors ran once, at boot, not once per
    // JS → Rust call (thousands happened above).
    assert_eq!(page.eval("__wasm.ctor_runs()"), json!(1), "static constructors ran exactly once");
}

#[test]
#[ignore = "compiles a crate for wasm32, runs wasm-bindgen and drives headless Chrome; run with --ignored"]
fn hybrid_module_runs_with_both_namespaces() {
    let site = scratch("hybrid-site");
    let report = build("own-glue-hybrid", Mode::Hybrid, true, &[], &site);
    assert!(report.package.bindgen_time.is_some());
    assert!(
        report.package.unwrapped_command_exports >= 5,
        "web-glue's four exports and wasm-bindgen's own were unwrapped: {}",
        report.package.unwrapped_command_exports
    );
    let bindgen_js = std::fs::read_to_string(site.join("pkg/own_glue_hybrid.js")).unwrap();
    assert!(
        bindgen_js.contains("./__idealyst_glue.js"),
        "wasm-bindgen passed the glue namespace through as an ES import"
    );
    assert!(site.join("pkg/__idealyst_glue.js").is_file());

    index_html(
        &site,
        "import init, { bindgen_greet, ctor_runs } from './pkg/own_glue_hybrid.js';\n \
         await init();\n window.__h = { bindgen_greet, ctor_runs };",
    );
    let mut page = open_site(&site, "hybrid");
    page.wait_until("hybrid boot", "window.__booted === true", Duration::from_secs(30));
    page.wait_until("hybrid timer", "document.getElementById('hybrid-async')?.textContent === 'timer 20'", Duration::from_secs(10));

    assert_eq!(page.text("hybrid-glue"), "from web-glue ✓");
    assert_eq!(page.text("hybrid-echo"), "echo from web-glue ✓", "JS → Rust string through web-glue's alloc");
    for _ in 0..5 {
        page.eval("document.getElementById('hybrid-btn').click()");
    }
    assert_eq!(page.text("hybrid-clicks"), "5 clicks");
    assert_eq!(page.eval("__h.bindgen_greet('glue')"), json!("hello glue from wasm-bindgen ✓"));
    assert_eq!(page.eval("__h.ctor_runs()"), json!(1), "static constructors ran exactly once");
}

// ---- measurements -----------------------------------------------------------

fn brotli_len(path: &Path) -> usize {
    let bytes = std::fs::read(path).unwrap();
    let mut out = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut out, 4096, 11, 22);
        w.write_all(&bytes).unwrap();
    }
    out.len()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Runs the workload `runs` times in the page and returns the median ms of
/// (create 10k rows with 3 attributes + text, update 10k texts).
fn bench(page: &mut Browser, api: &str, runs: usize) -> (f64, f64) {
    let (mut create, mut update) = (Vec::new(), Vec::new());
    // One warm-up round (JIT, allocator growth), not counted.
    for round in 0..=runs {
        let t = page.eval(&format!(
            "(() => {{ const a = performance.now(); {api}.bench_create(10000); const b = performance.now(); \
               {api}.bench_update(); const c = performance.now(); {api}.bench_clear(); return [b - a, c - b]; }})()"
        ));
        assert_eq!(t.as_array().map(Vec::len), Some(2), "bench round failed: {t}");
        if round > 0 {
            create.push(t[0].as_f64().unwrap());
            update.push(t[1].as_f64().unwrap());
        }
    }
    (median(create), median(update))
}

#[test]
#[ignore = "phase-1 measurements: builds both demos in release, runs wasm-bindgen and headless Chrome; run with --ignored --nocapture"]
fn measure_own_glue_against_web_sys() {
    const RUNS: usize = 9;
    let own_site = scratch("measure-own");
    let own = build("own-glue-demo", Mode::Own, true, &["--no-default-features"], &own_site);
    let bg_site = scratch("measure-bindgen");
    // A plain wasm-bindgen build: package_hybrid over a module with no glue
    // is exactly `wasm-bindgen --target web` plus a no-op extraction.
    let bg = build("own-glue-websys-demo", Mode::Hybrid, true, &[], &bg_site);

    let size = |site: &Path, lib: &str| {
        let wasm = site.join(format!("pkg/{lib}_bg.wasm"));
        let js = site.join(format!("pkg/{lib}.js"));
        let raw = std::fs::metadata(&wasm).unwrap().len() as usize + std::fs::metadata(&js).unwrap().len() as usize;
        (raw, brotli_len(&wasm) + brotli_len(&js), std::fs::metadata(&wasm).unwrap().len())
    };
    let (own_raw, own_br, own_wasm) = size(&own_site, "own_glue_demo");
    let (bg_raw, bg_br, bg_wasm) = size(&bg_site, "own_glue_websys_demo");
    // What a release `idealyst build --web` ships: wasm-opt -Oz with the
    // same flags `wasm_opt_pkg` uses, then brotli. Skipped without wasm-opt.
    let opt_br = |site: &Path, lib: &str| -> Option<usize> {
        let wasm = site.join(format!("pkg/{lib}_bg.wasm"));
        let out = site.join(format!("pkg/{lib}_bg.opt.wasm"));
        let st = Command::new("wasm-opt")
            .args(["-Oz", "--strip-debug", "--strip-producers", "--enable-bulk-memory", "--enable-nontrapping-float-to-int"])
            .arg("-o")
            .arg(&out)
            .arg(&wasm)
            .status()
            .ok()?;
        st.success().then(|| brotli_len(&out) + brotli_len(&site.join(format!("pkg/{lib}.js"))))
    };
    let own_opt = opt_br(&own_site, "own_glue_demo");
    let bg_opt = opt_br(&bg_site, "own_glue_websys_demo");

    index_html(&own_site, "import init from './pkg/own_glue_demo.js';\n window.__wasm = await init();");
    index_html(
        &bg_site,
        "import init, * as m from './pkg/own_glue_websys_demo.js';\n await init();\n window.__wasm = m;",
    );
    let mut own_page = open_site(&own_site, "measure-own");
    own_page.wait_until("own boot", "document.getElementById('status')?.textContent === 'ready'", Duration::from_secs(30));
    let (own_create, own_update) = bench(&mut own_page, "__wasm", RUNS);
    drop(own_page);
    let mut bg_page = open_site(&bg_site, "measure-bindgen");
    bg_page.wait_until("web-sys boot", "document.getElementById('status')?.textContent === 'ready'", Duration::from_secs(30));
    let (bg_create, bg_update) = bench(&mut bg_page, "__wasm", RUNS);

    println!("\n=== own-glue phase-1 measurements (release, medians of {RUNS} runs after 1 warm-up) ===");
    println!("                          own glue      web-sys+wasm-bindgen");
    println!(
        "cargo build (s)           {:>8.2}      {:>8.2}",
        own.cargo_time.as_secs_f64(),
        bg.cargo_time.as_secs_f64()
    );
    println!(
        "post-cargo pass (ms)      {:>8.1}      {:>8.1}   (glue extract+write | wasm-bindgen CLI + glue no-op)",
        own.package.pass_time.as_secs_f64() * 1e3,
        (bg.package.bindgen_time.unwrap() + bg.package.pass_time).as_secs_f64() * 1e3
    );
    println!("wasm bytes                {own_wasm:>8}      {bg_wasm:>8}");
    println!("wasm+js bytes             {own_raw:>8}      {bg_raw:>8}");
    println!("wasm+js brotli q11        {own_br:>8}      {bg_br:>8}");
    println!(
        "wasm-opt -Oz, then brotli  {:>8}      {:>8}",
        own_opt.map_or("-".into(), |n| n.to_string()),
        bg_opt.map_or("-".into(), |n| n.to_string())
    );
    println!("create 10k x (3 attr+text) ms {own_create:>6.2}      {bg_create:>8.2}");
    println!("update 10k texts ms       {own_update:>8.2}      {bg_update:>8.2}");
    println!(
        "glue imports {}, glue section {} bytes stripped",
        own.package.glue_imports, own.package.glue_section_bytes
    );
}
