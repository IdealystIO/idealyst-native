//! Serve the real marketing site over HTTP, server-side-rendered per
//! route.
//!
//!   cargo run -p website --example serve              # SSR + hydrate
//!   cargo run -p website --example serve -- --static  # SSR only, no boot
//!   cargo run -p website --example serve -- 127.0.0.1:8790 --static
//!   # then open http://127.0.0.1:8787/  /install  /concepts  …
//!
//! Each request renders `website::app()` at that URL and returns a
//! distinct, fully-styled document with the sidebar/footer chrome.
//!
//! Two modes:
//! - **default** — the page references `/pkg/website.js`, so the bundle
//!   boots and HYDRATES the server DOM (adopts it). Needs a built bundle
//!   (`idealyst build --web` → `dist/web`).
//! - **`--static`** — no `<script>` is emitted, so the bundle never
//!   boots and there is NO hydration. This is the pure server-render: you
//!   see exactly what the server paints (e.g. how the responsive
//!   media-query navigation renders) with nothing mutating it afterward.
//!   Fonts are still served so the first paint uses the real typeface.

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::{serve, ServeConfig};
use std::path::PathBuf;

fn main() {
    // First non-flag arg is the bind addr; `--static` (or bare `static`)
    // anywhere switches to no-hydration server-render-only mode.
    let mut addr = "127.0.0.1:8787".to_string();
    let mut static_only = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--static" | "static" => static_only = true,
            other if !other.starts_with('-') => addr = other.to_string(),
            _ => {}
        }
    }

    // Serve the built web bundle dir (`idealyst build --web` → dist/web):
    // it has `/pkg/website.js` (the hydrate-aware bundle) + `/fonts/*.ttf`.
    // Fonts are needed in BOTH modes so the SSR first paint uses the real
    // font rather than a fallback; the bundle is only referenced when we
    // emit the boot `<script>` (default mode).
    let static_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dist/web");

    // `None` → `render_document` omits the boot `<script>`, so the page is
    // a pure server render with no client takeover (no hydration). `Some`
    // → the bundle boots and adopts the server DOM.
    let bundle_module = if static_only {
        None
    } else {
        Some("/pkg/website.js".to_string())
    };

    if static_only {
        println!("SSR (static, no hydration) on http://{addr}");
    } else {
        println!("SSR + hydration on http://{addr}");
    }

    serve(
        &addr,
        ServeConfig {
            bundle_module,
            static_dir: Some(static_dir),
        },
        |b| {
            // Same extensions the web build registers (see
            // `website::register_extensions`) so SSR renders identically
            // and the bundle hydrates by adoption: navigator chrome +
            // the code-block external (server-rendered `<pre>`).
            drawer_navigator::chrome::register(b);
            codeblock::register(b);
        },
        website::app,
    )
    .expect("SSR server failed to start");
}
