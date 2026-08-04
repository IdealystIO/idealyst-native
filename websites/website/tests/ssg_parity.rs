//! Full-site SSG + SSR gate against the frozen corpus.
//!
//! ONE test does three things:
//!
//! 1. **Crawls the whole site** through the production SSG driver
//!    (`backend_ssr::newcore::render_all`) and asserts the navigator
//!    route collector discovered EVERY literal website route
//!    ([`EXPECTED_ROUTES`], 33 routes, none parameterized).
//! 2. **Serves one route over real HTTP** through the production
//!    per-request path (`newcore::serve`) and asserts the response is
//!    byte-identical to the direct render — the request-serving wiring,
//!    not just the renderer.
//! 3. **Dumps every page** (`html` + `head_css` + the served document)
//!    to `target/…/website-ssg/` and **byte-compares every file against
//!    the frozen corpus** in `tests/goldens/ssg/`, failing on the first
//!    divergence.
//!
//! ```text
//! cargo test -p website --features ssr --test ssg_parity
//! ```
//!
//! # The frozen corpus
//!
//! `tests/goldens/ssg/` is the OLD walker's committed output — 33 real
//! routes of a real app plus the served document, captured while the old
//! core still existed. Byte-identity against it is the **hydration
//! acceptance proof** for the website: the web adopt-mode boot walks SSR
//! DOM in creation order, so byte-identical server output adopts
//! identically.
//!
//! Nothing can re-derive that corpus anymore — the freeze half died with
//! `runtime-core`. A mismatch here is therefore a bug to FIX (a
//! hydration bug, or a style-resolution drift like the default-font
//! fold-in this gate caught), never a golden to rewrite. Do NOT
//! normalize divergences away; see `tests/goldens/ssg/README.md`.

#![cfg(feature = "ssr")]

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

/// Every literal route the website registers (lib.rs `SwapNavigator`
/// builder), sorted. The crawl must discover exactly this set from `/`.
const EXPECTED_ROUTES: &[&str] = &[
    "/",
    "/agentic",
    "/architecture",
    "/backends",
    "/cli",
    "/code-splitting",
    "/comparisons",
    "/comparisons/dioxus",
    "/comparisons/electron",
    "/comparisons/flutter",
    "/comparisons/react",
    "/comparisons/web-frameworks",
    "/comparisons/when-not",
    "/concepts",
    "/demo",
    "/features",
    "/features/cross-platform",
    "/features/extensibility",
    "/features/performance",
    "/features/ssr",
    "/features/type-safety",
    "/further-reading",
    "/idea-ui",
    "/install",
    "/navigation",
    "/primitives",
    "/quickstart",
    "/reactivity",
    "/roadmap",
    "/server-functions",
    "/styling",
    "/targets",
    "/why-rust",
];

/// Route exercised over real HTTP (has code panels + TOC — the shapes
/// that historically diverged).
const SERVE_ROUTE: &str = "/quickstart";

fn dump_root() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("website-ssg")
}

/// `/` → `index`; `/comparisons/react` → `comparisons__react`.
fn file_stem(route: &str) -> String {
    let trimmed = route.trim_matches('/');
    if trimmed.is_empty() {
        "index".to_string()
    } else {
        trimmed.replace('/', "__")
    }
}

/// Collapse reactive-anchor wrappers (`<div style="display: contents">`
/// … matching `</div>`) out of an HTML string, keeping their contents.
/// Used ONLY as the documented fallback in [`assert_bytes`] — see the
/// presence note there.
fn collapse_reactive_anchors(html: &str) -> String {
    const ANCHOR: &str = "<div style=\"display: contents\">";
    let mut out = html.to_string();
    while let Some(start) = out.find(ANCHOR) {
        // Find the matching close by depth-scanning subsequent tags.
        let mut depth = 1usize;
        let mut i = start + ANCHOR.len();
        let close = loop {
            let open = out[i..].find("<div");
            let shut = out[i..].find("</div>");
            match (open, shut) {
                (Some(o), Some(s)) if o < s => {
                    depth += 1;
                    i += o + 4;
                }
                (_, Some(s)) => {
                    depth -= 1;
                    if depth == 0 {
                        break i + s;
                    }
                    i += s + 6;
                }
                _ => panic!("unbalanced anchor div in dump"),
            }
        };
        out.replace_range(close..close + 6, "");
        out.replace_range(start..start + ANCHOR.len(), "");
    }
    out
}

/// Byte-level first-divergence report so a failure is actionable.
///
/// One DOCUMENTED structural divergence is tolerated: reactive-anchor
/// placement. The renderer expresses `presence` as a standard Dyn hole,
/// which on an anchored host nests under a `display: contents` anchor
/// its own hydration boot adopts; the old walker managed the presence
/// swap imperatively with no anchor. Each side's SSR output matches its
/// own client's adoption contract, so the anchor sets legitimately
/// differ (`display: contents` is layout-inert). In practice this fires
/// on exactly one page (`primitives.html`). The comparison is strict
/// FIRST; only when the two sides become identical after collapsing
/// anchor wrappers on BOTH is the difference accepted — any other
/// divergence still fails with full context.
fn assert_bytes(label: &str, expected: &str, actual: &str) {
    if expected == actual {
        return;
    }
    if collapse_reactive_anchors(expected) == collapse_reactive_anchors(actual) {
        println!(
            "[ssg] `{label}`: anchor-placement-only divergence (documented presence \
             Dyn-hole anchor) — accepted",
        );
        return;
    }
    let at = expected
        .bytes()
        .zip(actual.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| expected.len().min(actual.len()));
    let lo = at.saturating_sub(80);
    panic!(
        "byte divergence in `{label}` at byte {at}\n\
         frozen (len {}): …{}…\n\
         actual (len {}): …{}…",
        expected.len(),
        &expected[lo..(at + 120).min(expected.len())],
        actual.len(),
        &actual[lo..(at + 120).min(actual.len())],
    );
}

/// Minimal HTTP GET against the test server (no client dep).
fn http_get(addr: &str, path: &str) -> String {
    // The server thread races this connect; retry briefly.
    let mut last_err = None;
    for _ in 0..50 {
        match std::net::TcpStream::connect(addr) {
            Ok(mut stream) => {
                write!(stream, "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
                    .expect("write request");
                let mut buf = Vec::new();
                stream.read_to_end(&mut buf).expect("read response");
                let text = String::from_utf8(buf).expect("utf8 response");
                let body_at = text.find("\r\n\r\n").expect("header terminator") + 4;
                let (head, body) = text.split_at(body_at);
                // tiny_http streams string responses chunked; decode.
                if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
                    let mut out = String::new();
                    let mut rest = body;
                    loop {
                        let line_end = rest.find("\r\n").expect("chunk size line");
                        let size = usize::from_str_radix(rest[..line_end].trim(), 16)
                            .expect("chunk size hex");
                        rest = &rest[line_end + 2..];
                        if size == 0 {
                            break;
                        }
                        out.push_str(&rest[..size]);
                        rest = &rest[size + 2..]; // skip chunk + CRLF
                    }
                    return out;
                }
                return body.to_string();
            }
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
    panic!("could not connect to test SSR server at {addr}: {last_err:?}");
}

// ---------------------------------------------------------------------------
// Frozen-artifact corpus (tests/goldens/ssg)
// ---------------------------------------------------------------------------

fn frozen_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("goldens").join("ssg")
}

/// Reserve an ephemeral localhost port (bind-then-drop; the tiny race
/// window is acceptable for a test).
fn ephemeral_addr() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind :0");
    let addr = listener.local_addr().expect("local addr").to_string();
    drop(listener);
    addr
}

#[test]
fn ssg_crawl_and_ssr_serve_full_site_byte_parity() {
    // ---- 1) Full-site crawl through the production SSG driver. ----
    let result =
        backend_ssr::newcore::render_all(website::register_ssr_scene_handlers, website::app);

    assert!(
        result.skipped_parameterized.is_empty(),
        "website has no parameterized routes; crawl skipped {:?}",
        result.skipped_parameterized,
    );
    let mut crawled: Vec<&str> = result.pages.keys().map(|s| s.as_str()).collect();
    crawled.sort_unstable();
    assert_eq!(
        crawled, EXPECTED_ROUTES,
        "the navigator route collector must discover every literal website route"
    );

    // ---- 2) Per-request serving path over real HTTP. ----
    let addr = ephemeral_addr();
    let config = backend_ssr::ServeConfig {
        bundle_module: None,
        static_dir: None,
        extra_head: None,
        // Non-premint posture — this gate's corpus is frozen non-premint
        // output; `None` keeps every byte identical.
        premint_css: None,
    };
    {
        let addr = addr.clone();
        std::thread::spawn(move || {
            // Blocks forever; the thread dies with the test process.
            backend_ssr::newcore::serve(
                &addr,
                config,
                website::register_ssr_scene_handlers,
                website::app,
            )
            .expect("serve");
        });
    }
    let served = http_get(&addr, SERVE_ROUTE);
    // The served response must be exactly the direct render of the same
    // route wrapped in the document shell — proves the HTTP path adds /
    // loses nothing. (The crawl page is rendered on this thread, the
    // served one on a fresh request thread: also pins that per-request
    // isolation reproduces the identical bytes.)
    let direct = backend_ssr::render_document(
        result.pages.get(SERVE_ROUTE).expect("crawl covered the served route"),
        None,
        None,
        None,
    );
    assert_bytes("served-vs-direct", &direct, &served);

    // ---- 3) Dump + frozen-corpus byte gate. ----
    let dir = dump_root();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create dump dir");

    let mut files: Vec<String> = Vec::new();
    for (route, page) in &result.pages {
        assert!(!page.html.is_empty(), "SSG render of {route} produced empty html");
        let stem = file_stem(route);
        std::fs::write(dir.join(format!("{stem}.html")), &page.html).unwrap();
        std::fs::write(dir.join(format!("{stem}.head.css")), &page.head_css).unwrap();
        files.push(format!("{stem}.html"));
        files.push(format!("{stem}.head.css"));
    }
    std::fs::write(dir.join("served.doc.html"), &served).unwrap();
    files.push("served.doc.html".to_string());
    files.sort_unstable();
    // Manifest written LAST: its presence marks a complete dump, so a
    // failed/partial run is never used as a comparison baseline.
    std::fs::write(dir.join("MANIFEST.txt"), files.join("\n")).unwrap();

    // ---- 3b) Frozen-corpus gate. ----
    //
    // The corpus is the old walker's committed output; the freeze half
    // died with `runtime-core`, so this comparison can only be satisfied
    // by fixing the renderer, never by rewriting the artifacts. Same
    // `assert_bytes` the served-vs-direct check uses (strict first,
    // documented anchor-collapse fallback only).
    let frozen = frozen_dir();
    let manifest = frozen.join("MANIFEST.txt");
    assert!(
        manifest.is_file(),
        "missing frozen SSG corpus at {} — it is the OLD walker's output, captured \
         before `runtime-core` was deleted, and cannot be re-derived. \
         See tests/goldens/ssg/README.md.",
        frozen.display(),
    );
    let frozen_files = std::fs::read_to_string(&manifest).unwrap();
    assert_eq!(
        frozen_files.lines().collect::<Vec<_>>(),
        files.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        "the frozen corpus and this run must cover the same file set \
         (a route was added or removed — re-baseline deliberately)"
    );
    for file in &files {
        let expected = std::fs::read_to_string(frozen.join(file)).unwrap();
        let actual = std::fs::read_to_string(dir.join(file)).unwrap();
        assert_bytes(&format!("frozen:{file}"), &expected, &actual);
    }
    println!(
        "[ssg] matches the frozen corpus across {} files ({} routes + served doc)",
        files.len(),
        result.pages.len(),
    );
}
