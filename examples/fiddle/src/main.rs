//! Fiddle server — serves the Idealyst-built editor UI, accepts
//! source via `POST /compile`, runs wasm-pack against the template
//! crate, and serves the resulting bundle from the on-disk cache
//! (keyed by sha256 of the source).
//!
//! Threading model: tiny_http is sync. Each request runs on a worker
//! thread spawned by the server loop. Compilation itself is
//! serialized via a `Mutex<()>` because cargo writes to a single
//! `target/` directory and concurrent invocations would step on
//! each other.
//!
//! Layout (rooted at the workspace's `examples/fiddle/`):
//!
//! - `webapp/`         — Idealyst-built cdylib (the editor UI). Build
//!                       with `wasm-pack build webapp/ --target web
//!                       --dev` before launching the server; the
//!                       server serves the resulting `pkg/` directly.
//! - `template/`       — wrapping crate for the user's snippet. Its
//!                       `src/snippet.rs` is overwritten per compile;
//!                       `src/lib.rs` hosts the rendered snippet
//!                       inside a `host-web` simulator (so the iframe
//!                       paints the iOS-skinned chrome the docs site
//!                       does, just around the user's snippet). It's
//!                       its own `[workspace]` so cargo doesn't try
//!                       to merge it into the parent workspace's
//!                       `target/`.
//! - `compiled/<hash>/`— per-source-hash output cache. Each directory
//!                       contains the wasm bundle + an `index.html`
//!                       shim. Served at `/compiled/<hash>/...`.

mod compile;
mod server;

use anyhow::Result;

fn main() -> Result<()> {
    server::run("0.0.0.0", 8081)
}
