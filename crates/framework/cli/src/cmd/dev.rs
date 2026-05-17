//! `idealyst dev` — wire `dev-reload` and `dev-http` together (reload
//! mode), or hand off to the `dev-server` crate's wire protocol (AAS
//! mode). This module is intentionally thin: it parses flags,
//! resolves the project dir, and dispatches.

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use anyhow::Result;
use dev_http::{serve_static, ReloadContext};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Project directory to serve. This is the dir containing the
    /// `Cargo.toml` (and `index.html`) — typically the same place
    /// you'd run `wasm-pack build` by hand.
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// TCP port the HTTP server binds on.
    #[arg(long, default_value_t = 9000)]
    pub port: u16,

    /// Host/interface to bind on. Loopback by default; pass
    /// `0.0.0.0` to expose to the LAN (handy for testing on a phone).
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// Development mode. See [`Mode`] for the trade-offs between
    /// rebuild-and-reload vs application-as-a-server.
    #[arg(long, value_enum, default_value_t = Mode::Reload)]
    pub mode: Mode,

    /// Skip the initial build and the watch loop. Useful when you've
    /// already produced `pkg/` and just want a static server — or
    /// when iterating on something the watcher can't see (e.g. assets
    /// produced by a sibling tool).
    #[arg(long)]
    pub no_build: bool,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Mode {
    /// **Rebuild + browser reload.** Source change → `wasm-pack build`
    /// → connected browsers reload the page. Heaviest per-iteration
    /// cost (full wasm compile), but the simplest mental model: every
    /// reload is a fresh process exactly like production.
    Reload,
    /// **Application-as-a-Server.** The app's reactive runtime lives
    /// on a dev-host process; the browser is a thin client that
    /// streams primitive commands over a WebSocket and ships
    /// interactions back. Source changes only rebuild the dev-host
    /// binary — no wasm-pack, no page reload, signal state survives.
    /// Costs: every primitive and event round-trips the wire; some
    /// platform-specific code (e.g. wgpu surfaces) can't be driven
    /// from the server side.
    Aas,
}

pub fn run(args: Args) -> Result<()> {
    let dir = std::fs::canonicalize(&args.dir)
        .map_err(|e| anyhow::anyhow!("cannot resolve project dir {}: {e}", args.dir.display()))?;
    match args.mode {
        Mode::Reload => run_reload(&dir, &args.host, args.port, args.no_build),
        Mode::Aas => run_aas(&dir, &args.host, args.port),
    }
}

fn run_reload(
    dir: &std::path::Path,
    host: &str,
    port: u16,
    no_build: bool,
) -> Result<()> {
    let gen = Arc::new(AtomicU64::new(0));

    // `dev-reload` runs the initial build synchronously, then keeps
    // its watcher thread alive in the returned `JoinHandle`. We
    // forget the handle: it's tied to the HTTP server's lifetime,
    // and there's no clean teardown path when Ctrl-C arrives anyway.
    if !no_build {
        let handle = dev_reload::start(dir, gen.clone())?;
        std::mem::forget(handle);
    }

    let ctx = ReloadContext { gen };
    serve_static(host, port, dir, Some(ctx))
}

fn run_aas(_dir: &std::path::Path, _host: &str, _port: u16) -> Result<()> {
    // The framework's `dev-server` crate already provides the wire
    // recording backend, the WebSocket server, and the cargo-rebuild
    // + exec loop. What's missing is the project-side wrapper that
    // wires the user's `app()` function into it.
    //
    // Sketch:
    //   1. Look up `[package.metadata.idealyst.dev.aas_bin]` in the
    //      project's `Cargo.toml` (default: `dev-server`). If the
    //      crate doesn't define that binary, fall through to a
    //      generated wrapper under `target/idealyst/aas-host/` that
    //      imports the user's `app` fn and hosts it.
    //   2. `cargo build --release -p <project> --bin <aas_bin>` to
    //      produce the host binary.
    //   3. `Command::new(<host_bin>).spawn()`; it WebSocket-listens
    //      on a port (default 9001).
    //   4. Run our HTTP server alongside, serving an `index.html`
    //      that loads a wasm shim built once with the framework's
    //      `dev-client::web` feature — this shim is what connects
    //      to the host, applies wire commands, and ships events
    //      back.
    //   5. Source change → host binary rebuild → `exec` (the
    //      `dev-server` crate already does this and snapshots nav
    //      state across the swap). Browser auto-reconnects via the
    //      `connect_attempt` retry loop already in our web shim.
    //
    // Tracked separately because each step needs ergonomic CLI
    // surface (where the shim lives, how the project opts in, what
    // the regenerated wrapper looks like under `target/idealyst/`).
    anyhow::bail!(
        "AAS mode is not implemented yet — wire-up tracked in cmd::dev::run_aas. \
         For now use `--mode reload` (the default)."
    )
}
