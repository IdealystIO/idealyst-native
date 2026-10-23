//! The fiddle server: compiles submitted Idealyst code into a real web
//! app and serves it, for the tutorial's live examples and for the
//! standalone editor in `webapp/`.
//!
//! - `POST /compile` takes `{ "files": { "app.rs": "...", ... } }` and
//!   answers with a built bundle's URL or the compiler's diagnostics
//!   (see [`compile::Outcome`]).
//! - `GET /compiled/<hash>/` serves a built bundle; it is what a preview
//!   `<iframe>` loads.
//! - `GET /` and `/pkg/*` serve the standalone editor, once built with
//!   `idealyst build --web examples/fiddle/webapp` (to `webapp/dist/web`).
//!
//! Every response allows any origin, so a page served elsewhere (the
//! tutorial under `idealyst dev`) can call it.
//!
//! Configuration, all optional:
//! - `FIDDLE_ADDR` — listen address (default `127.0.0.1:8787`; not 8081,
//!   which `idealyst dev --ssr` takes by default).
//! - `IDEALYST_BIN` — the CLI that builds bundles (default `idealyst` on
//!   `PATH`). Point it at a CLI built from this checkout
//!   (`target/debug/idealyst`) when the installed one is older than the
//!   framework source the fiddle builds against.
//!
//! The server compiles and runs whatever it is sent, with the permissions
//! of the user running it — a build script or proc macro in submitted code
//! runs on the host. Serve it on localhost only, or behind a sandbox.

mod compile;
mod server;

use std::path::PathBuf;

use anyhow::Result;

fn main() -> Result<()> {
    let addr = std::env::var("FIDDLE_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".to_string());
    let cli = std::env::var_os("IDEALYST_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("idealyst"));
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let compiler = compile::Compiler::new(root.clone(), cli)?;
    server::run(&addr, root, compiler)
}
