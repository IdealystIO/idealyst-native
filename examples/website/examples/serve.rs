//! Serve the real marketing site over HTTP, server-side-rendered per
//! route.
//!
//!   cargo run -p website --example serve
//!   # then open http://127.0.0.1:8787/  /install  /concepts  …
//!
//! Each request renders `website::app()` at that URL and returns a
//! distinct, fully-styled document with the sidebar/footer chrome. The
//! page boots `/pkg/website.js` to hydrate — set `static_dir` to a built
//! web bundle (`idealyst build --web`) to serve it; left `None` here, so
//! this is a server-rendered content/SEO preview (pages render, the
//! bundle 404s, no hydration).

#![cfg(not(target_arch = "wasm32"))]

use backend_ssr::{serve, ServeConfig};
use std::path::PathBuf;

fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:8787".into());
    // Serve the website crate dir so the SSR pages can fetch the assets
    // they link: `/fonts/Inter-*.ttf` (always present — needed for the
    // first paint to use the real font, not a fallback) and the web
    // bundle `/pkg/website.js` (present after `idealyst build --web` /
    // `wasm-pack` — enables hydration). Without this the pages render but
    // text falls back to a system font and the bundle 404s.
    let static_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    serve(
        &addr,
        ServeConfig {
            // Pure SSR preview: just transmit the rendered screen (no
            // hydration). Set this to `Some("/pkg/website.js")` after
            // `idealyst build --web` to also boot the bundle and hydrate —
            // it replaces the server-rendered DOM on load.
            bundle_module: None,
            // Serve `/fonts/*.ttf` so the first paint uses the real font
            // (and `/pkg/*` when hydration is enabled above).
            static_dir: Some(static_dir),
        },
        |b| drawer_navigator::chrome::register(b),
        website::app,
    )
    .expect("SSR server failed to start");
}
