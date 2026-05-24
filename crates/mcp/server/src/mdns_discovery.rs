//! mDNS-based discovery of running Idealyst apps.
//!
//! The server browses `_idealyst-robot._tcp.local.` on a background
//! thread and maintains an in-memory map of currently-live apps
//! keyed by their `app` TXT-record value. When an app's mDNS service
//! is removed (graceful shutdown or TTL expiry), the map entry goes
//! with it — no stale `(name, addr)` pairs surviving past app exit.
//!
//! This complements (rather than replaces) `~/.idealyst/registry.json`.
//! The registry stays as the long-form record carrying catalog-bin
//! paths and survives mDNS-hostile network environments (corporate
//! VPN, blocked multicast). The MCP server consults mDNS first and
//! the registry second.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Service type advertised by the running app's bridge — must match
/// `runtime_core::robot::bridge::MDNS_SERVICE_TYPE`.
pub const SERVICE_TYPE: &str = "_idealyst-robot._tcp.local.";

/// One live app as discovered via mDNS.
#[derive(Debug, Clone)]
pub struct DiscoveredApp {
    /// `app` TXT-record value — the `runtime_core::AppIdentity::name`.
    pub name: String,
    /// `bundle_id` TXT-record value, if any.
    pub bundle_id: Option<String>,
    /// `project_root` TXT-record value, if any. May be empty when
    /// the path was too long to fit in the TXT record (see
    /// `runtime_core::robot::bridge`'s advertising code).
    pub project_root: Option<String>,
    /// `catalog_bin` TXT-record value, if any. The server uses this
    /// as a fallback when the bridge's `get_catalog` command isn't
    /// available (e.g. older app versions).
    pub catalog_bin: Option<String>,
    /// `pid` TXT-record value, parsed.
    pub pid: u32,
    /// Bridge socket address — `<ip>:<port>` where the Robot bridge
    /// is listening.
    pub bridge_addr: String,
}

/// Live, lock-protected map of `name → DiscoveredApp`. Cheap to
/// clone — the inner is an `Arc<Mutex<...>>`.
#[derive(Clone, Default)]
pub struct DiscoveryTable {
    inner: Arc<Mutex<HashMap<String, DiscoveredApp>>>,
}

impl DiscoveryTable {
    /// Look up a service by its `app` TXT key. Returns a snapshot —
    /// safe to use even if the entry vanishes mid-call.
    pub fn get(&self, name: &str) -> Option<DiscoveredApp> {
        self.inner.lock().ok()?.get(name).cloned()
    }

    /// Snapshot of every currently-known app. Sorted by name for
    /// deterministic `list_apps` output.
    pub fn snapshot(&self) -> Vec<DiscoveredApp> {
        let Ok(guard) = self.inner.lock() else {
            return Vec::new();
        };
        let mut out: Vec<DiscoveredApp> = guard.values().cloned().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

/// Start a background thread that browses for `_idealyst-robot._tcp`
/// and keeps the [`DiscoveryTable`] up to date. Returns the table
/// immediately; the thread runs for the lifetime of the process.
///
/// Browser failure (daemon init error, no network) returns an empty
/// table that simply never populates. Callers consult the registry
/// as fallback in that case.
pub fn start() -> DiscoveryTable {
    let table = DiscoveryTable::default();
    let table_for_thread = table.clone();

    std::thread::Builder::new()
        .name("idealyst-mdns-browser".into())
        .spawn(move || run_browser(table_for_thread))
        .ok();

    table
}

fn run_browser(table: DiscoveryTable) {
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("mDNS daemon init failed ({}); discovery limited to registry", e);
            return;
        }
    };
    let receiver = match daemon.browse(SERVICE_TYPE) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("mDNS browse failed ({}); discovery limited to registry", e);
            return;
        }
    };

    // Browser events are pushed onto the receiver; we just drain
    // them and react. The crate handles re-querying / TTL / removals.
    while let Ok(event) = receiver.recv() {
        match event {
            mdns_sd::ServiceEvent::ServiceResolved(info) => {
                if let Some(app) = parse_service(&info) {
                    if let Ok(mut guard) = table.inner.lock() {
                        guard.insert(app.name.clone(), app);
                    }
                }
            }
            mdns_sd::ServiceEvent::ServiceRemoved(_type, fullname) => {
                // The fullname is `<instance>.<service-type>`. We
                // keyed the table by `app` (from TXT), not the
                // instance name — so we have to walk the table to
                // find the matching entry. Cheap; the table is tiny.
                if let Ok(mut guard) = table.inner.lock() {
                    guard.retain(|_, v| !fullname.starts_with(&format!("{}-{}", v.name.replace('.', "-"), v.pid)));
                }
            }
            _ => {}
        }
    }
}

fn parse_service(info: &mdns_sd::ServiceInfo) -> Option<DiscoveredApp> {
    // mdns-sd's TXT iterator hands back `(key, value)` pairs as
    // borrowed slices/strs. The values can contain arbitrary bytes
    // but for our records they're all UTF-8 strings the framework
    // emitted.
    let mut name: Option<String> = None;
    let mut bundle_id: Option<String> = None;
    let mut project_root: Option<String> = None;
    let mut catalog_bin: Option<String> = None;
    let mut pid: u32 = 0;
    for prop in info.get_properties().iter() {
        let k = prop.key();
        let v = prop.val_str();
        match k {
            "app" => {
                if !v.is_empty() {
                    name = Some(v.to_string());
                }
            }
            "bundle_id" => {
                if !v.is_empty() {
                    bundle_id = Some(v.to_string());
                }
            }
            "project_root" => {
                if !v.is_empty() {
                    project_root = Some(v.to_string());
                }
            }
            "catalog_bin" => {
                if !v.is_empty() {
                    catalog_bin = Some(v.to_string());
                }
            }
            "pid" => {
                if let Ok(n) = v.parse::<u32>() {
                    pid = n;
                }
            }
            _ => {}
        }
    }
    let name = name?;
    // Pick the first reachable address. The bridge binds 0.0.0.0,
    // so any resolved address should work. Prefer IPv4 for the
    // typical dev setup (Android emulator's NAT routes IPv4 from
    // the host).
    let addr = info
        .get_addresses()
        .iter()
        .find(|a| a.is_ipv4())
        .copied()
        .or_else(|| info.get_addresses().iter().next().copied())?;
    let port = info.get_port();
    let bridge_addr = format!("{}:{}", addr, port);

    Some(DiscoveredApp {
        name,
        bundle_id,
        project_root,
        catalog_bin,
        pid,
        bridge_addr,
    })
}
