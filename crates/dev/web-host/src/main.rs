//! Dev-time HTTP host for web bundles.
//!
//! Two responsibilities:
//!
//! 1. **Static file server.** Serves `index.html` + wasm-pack output
//!    out of a configured root. Delegates to [`dev_http::serve_static`]
//!    so we inherit SPA fallback, no-cache headers, and correct
//!    MIME types.
//!
//! 2. **runtime-server URL injection.** Reads the dev-server's bound
//!    port from a port-file sentinel the dev-host writes
//!    (`--port-file`), constructs `ws://localhost:<port>`, and feeds
//!    it into [`dev_http::AasContext`]. The HTTP layer then (a)
//!    inlines a `<script>window.IDEALYST_RUNTIME_SERVER_URL = "..."</script>`
//!    into every served HTML response and (b) exposes the same value
//!    via `/__idealyst/aas_url`. Wasm bundles pick it up synchronously
//!    at boot, no async fetch dance required.
//!
//! Replaces the old `serve.py` — that script only did the static
//! side, so users had to bake the runtime-server URL into the wasm at build
//! time. This binary closes the loop: a dev-server that restarts
//! on a fresh port is transparently picked up by a page refresh
//! (the port file gets rewritten by the new host on bind).
//!
//! Usage:
//!
//! ```text
//! web-dev-host \
//!     --addr 0.0.0.0:8080 \
//!     --root examples/hello-world/pkg \
//!     --port-file /path/to/host-port
//! ```
//!
//! Arguments are positional/long-named only; deliberately no clap
//! dep — keeps the dev-tooling compile time tight.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use dev_http::{serve_static, AasContext};

struct Args {
    addr: String,
    host: String,
    port: u16,
    root: PathBuf,
    port_file: PathBuf,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    eprintln!(
        "[web-dev-host] root={} addr={} port-file={}",
        args.root.display(),
        args.addr,
        args.port_file.display(),
    );

    let aas_url: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    spawn_port_file_watcher(args.port_file.clone(), aas_url.clone());

    let ctx = AasContext {
        aas_url: aas_url.clone(),
    };

    serve_static(&args.host, args.port, &args.root, None, Some(ctx), None)
}

fn parse_args() -> Result<Args> {
    let mut addr = String::from("0.0.0.0:8080");
    let mut root = PathBuf::from(".");
    let mut port_file: Option<PathBuf> = None;

    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--addr" => {
                addr = raw
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--addr requires a value"))?
                    .clone();
                i += 2;
            }
            "--root" => {
                root = PathBuf::from(
                    raw.get(i + 1)
                        .ok_or_else(|| anyhow!("--root requires a value"))?,
                );
                i += 2;
            }
            "--port-file" => {
                port_file = Some(PathBuf::from(
                    raw.get(i + 1)
                        .ok_or_else(|| anyhow!("--port-file requires a value"))?,
                ));
                i += 2;
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                anyhow::bail!("unknown argument {:?}", other);
            }
        }
    }

    let port_file = port_file.ok_or_else(|| {
        anyhow!("missing required --port-file <path> (the dev-host writes its bound port here)")
    })?;

    let (host, port) = parse_addr(&addr)?;
    Ok(Args {
        addr,
        host,
        port,
        root,
        port_file,
    })
}

fn parse_addr(addr: &str) -> Result<(String, u16)> {
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or_else(|| anyhow!("--addr must be host:port (got {addr:?})"))?;
    let port: u16 = port
        .parse()
        .with_context(|| format!("invalid port in --addr {addr:?}"))?;
    Ok((host.to_string(), port))
}

fn print_usage() {
    eprintln!(
        "Usage: web-dev-host --port-file <path> [--addr <host:port>] [--root <dir>]\n\
         \n\
         Static-file dev-host that reads the runtime-server dev-server's bound port\n\
         from a sentinel file and exposes `ws://localhost:<port>` to served pages\n\
         via `window.IDEALYST_RUNTIME_SERVER_URL` + `/__idealyst/aas_url`.\n\
         \n\
         Options:\n\
         \x20 --port-file <path>  Path to the dev-host port sentinel (required)\n\
         \x20 --addr <host:port>  HTTP bind address (default 0.0.0.0:8080)\n\
         \x20 --root <dir>        Static root directory (default current dir)"
    );
}

/// Background thread that watches `port_file` for changes and pushes
/// `ws://localhost:<port>` into the shared mutex. The dev-host
/// rewrites the file on every bind (including post-restart respawns),
/// so re-polling gives us live URL updates without restarting the
/// web-host. mtime-based — cheap enough to poll every 250ms.
fn spawn_port_file_watcher(port_file: PathBuf, out: Arc<Mutex<Option<String>>>) {
    thread::spawn(move || {
        let mut last_mtime: Option<std::time::SystemTime> = None;
        loop {
            let mtime = std::fs::metadata(&port_file)
                .ok()
                .and_then(|m| m.modified().ok());
            if mtime != last_mtime {
                last_mtime = mtime;
                let new_url = std::fs::read_to_string(&port_file)
                    .ok()
                    .and_then(|s| s.trim().parse::<u16>().ok())
                    .map(|p| format!("ws://127.0.0.1:{p}"));
                if let Some(url) = new_url.as_ref() {
                    eprintln!("[web-dev-host] dev-server at {url}");
                }
                if let Ok(mut g) = out.lock() {
                    *g = new_url;
                }
            }
            thread::sleep(Duration::from_millis(250));
        }
    });
}
