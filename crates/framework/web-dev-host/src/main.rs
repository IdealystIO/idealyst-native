//! Dev-time HTTP host for web bundles.
//!
//! Two responsibilities:
//!
//! 1. **Static file server.** Serves `index.html` + wasm-pack output
//!    out of a configured root. Delegates to [`dev_http::serve_static`]
//!    so we inherit SPA fallback, no-cache headers, and correct
//!    MIME types.
//!
//! 2. **AAS discovery bridge.** Browses the local network for an
//!    AAS dev-server matching a given `app_id` and feeds the
//!    resulting `ws://...` URL into [`dev_http::AasContext`]. The
//!    HTTP layer then (a) inlines a `<script>window.IDEALYST_AAS_URL
//!    = "..."</script>` into every served HTML response and
//!    (b) exposes the same value via `/__idealyst/aas_url`. Wasm
//!    bundles pick it up synchronously at boot, no async fetch
//!    dance required.
//!
//! Replaces the old `serve.py` — that script only did the static
//! side, so users had to bake the AAS URL into the wasm at build
//! time. This binary closes the loop: a dev-server that restarts
//! on a fresh ephemeral port is transparently picked up by a page
//! refresh.
//!
//! Usage:
//!
//! ```text
//! web-dev-host \
//!     --addr 0.0.0.0:8080 \
//!     --root examples/hello-web \
//!     --app-id hot-reload-demo
//! ```
//!
//! Arguments are positional/long-named only; deliberately no clap
//! dep — keeps the dev-tooling compile time tight.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Context, Result};
use dev_http::{serve_static, AasContext};
use mdns_sd::{ServiceDaemon, ServiceEvent};

/// DNS-SD service type the AAS dev-server publishes itself under.
/// Mirrors the constant in `dev-server/src/transport.rs`. Kept in
/// sync by convention rather than a shared crate to avoid this
/// binary having to pull in the whole `wire` + `dev-server`
/// dependency tree.
const AAS_SERVICE_TYPE: &str = "_idealyst-dev._tcp.local.";

struct Args {
    addr: String,
    host: String,
    port: u16,
    root: PathBuf,
    app_id: String,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    eprintln!(
        "[web-dev-host] root={} addr={} app_id={:?}",
        args.root.display(),
        args.addr,
        args.app_id
    );

    let aas_url: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    spawn_mdns_browser(args.app_id.clone(), aas_url.clone());

    let ctx = AasContext {
        aas_url: aas_url.clone(),
    };

    serve_static(&args.host, args.port, &args.root, None, Some(ctx))
}

fn parse_args() -> Result<Args> {
    let mut addr = String::from("0.0.0.0:8080");
    let mut root = PathBuf::from(".");
    let mut app_id: Option<String> = None;

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
            "--app-id" => {
                app_id = Some(
                    raw.get(i + 1)
                        .ok_or_else(|| anyhow!("--app-id requires a value"))?
                        .clone(),
                );
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

    let app_id = app_id.ok_or_else(|| {
        anyhow!("missing required --app-id <id> (matches the dev-server's mDNS TXT app_id)")
    })?;

    let (host, port) = parse_addr(&addr)?;
    Ok(Args {
        addr,
        host,
        port,
        root,
        app_id,
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
        "Usage: web-dev-host --app-id <id> [--addr <host:port>] [--root <dir>]\n\
         \n\
         Static-file dev-host that auto-discovers an AAS dev-server with\n\
         the matching `app_id` via Bonjour and exposes its URL to served\n\
         pages via `window.IDEALYST_AAS_URL` + `/__idealyst/aas_url`.\n\
         \n\
         Options:\n\
         \x20 --app-id <id>       AAS app_id to look for (required)\n\
         \x20 --addr <host:port>  HTTP bind address (default 0.0.0.0:8080)\n\
         \x20 --root <dir>        Static root directory (default current dir)"
    );
}

/// Long-lived mDNS browser thread. Watches `_idealyst-dev._tcp.`
/// for services whose TXT record's `app_id` matches ours. Writes
/// the resolved `ws://...` URL into the shared mutex; clears it
/// when the matching service disappears.
///
/// Keeping the daemon alive (vs. building one per discover() call)
/// matters here — we want the running daemon to *immediately*
/// notice when the AAS server restarts on a fresh port, so a page
/// refresh after a dev-server hot-reload sees the new URL.
fn spawn_mdns_browser(app_id: String, out: Arc<Mutex<Option<String>>>) {
    thread::spawn(move || {
        let daemon = match ServiceDaemon::new() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[web-dev-host] mDNS daemon init failed: {e}");
                return;
            }
        };
        let receiver = match daemon.browse(AAS_SERVICE_TYPE) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[web-dev-host] mDNS browse failed: {e}");
                return;
            }
        };
        eprintln!(
            "[web-dev-host] mDNS browsing {} for app_id={:?}",
            AAS_SERVICE_TYPE, app_id
        );
        for event in receiver.iter() {
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    if !txt_matches(&info, &app_id) {
                        continue;
                    }
                    let host_url = pick_url(&info);
                    if let Some(url) = host_url {
                        eprintln!("[web-dev-host] discovered AAS at {url}");
                        if let Ok(mut g) = out.lock() {
                            *g = Some(url);
                        }
                    }
                }
                ServiceEvent::ServiceRemoved(_, name) => {
                    eprintln!("[web-dev-host] AAS service {name} disappeared");
                    if let Ok(mut g) = out.lock() {
                        *g = None;
                    }
                }
                _ => {}
            }
        }
        eprintln!("[web-dev-host] mDNS browse loop exited");
    });
}

fn txt_matches(info: &mdns_sd::ServiceInfo, expected_app_id: &str) -> bool {
    for prop in info.get_properties().iter() {
        if prop.key().eq_ignore_ascii_case("app_id") && prop.val_str() == expected_app_id {
            return true;
        }
    }
    false
}

/// Pick the best address from a discovered service and format it as
/// a `ws://host:port` URL. Prefer IPv4 — tungstenite handles those
/// cleanly and dev LANs are almost always v4. Fall back to v6 with
/// the URL-required bracket form if no v4 is advertised.
fn pick_url(info: &mdns_sd::ServiceInfo) -> Option<String> {
    let port = info.get_port();
    let addrs = info.get_addresses();
    if let Some(v4) = addrs.iter().find(|a| a.is_ipv4()) {
        Some(format!("ws://{v4}:{port}"))
    } else if let Some(v6) = addrs.iter().next() {
        Some(format!("ws://[{v6}]:{port}"))
    } else {
        None
    }
}
