//! Filesystem-based discovery of running Idealyst apps.
//!
//! The Robot bridge inside each running app writes
//! `~/.idealyst/apps/<name>-<pid>.json` on bind (and removes it via
//! RAII on graceful shutdown). The MCP server scans that directory
//! to populate an in-memory map of currently-live apps keyed by app
//! name.
//!
//! Replaces the old mDNS / `_idealyst-robot._tcp.local.` discovery —
//! the file-based path avoids multicast firewall headaches on
//! corporate / VPN networks and produces deterministic results
//! across rerun cycles.
//!
//! Liveness check: each scan does `kill(pid, 0)` (a no-op syscall
//! that fails with ESRCH when the process is gone) to filter ghost
//! entries that a crash left behind without RAII running. Stale files
//! are deleted at scan time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One live app as discovered via the per-process registration file.
#[derive(Debug, Clone)]
pub struct DiscoveredApp {
    /// `name` field from the JSON — the
    /// [`runtime_core::robot::bridge::AppIdentity::name`].
    pub name: String,
    /// `bundle_id` from the JSON, if any.
    pub bundle_id: Option<String>,
    /// `project_root` from the JSON, if any.
    pub project_root: Option<String>,
    /// Catalog-bin path — populated out-of-band today (the bridge JSON
    /// doesn't carry it). Future revision can either embed it in the
    /// registration file or have the MCP server query the bridge's
    /// `get_identity` command for it.
    pub catalog_bin: Option<String>,
    /// `pid` from the JSON.
    pub pid: u32,
    /// `<host>:<port>` where the Robot bridge is listening. Always
    /// `127.0.0.1:<port>` — the bridge binds `0.0.0.0` but the MCP
    /// server runs on the same machine.
    pub bridge_addr: String,
}

/// Live, lock-protected map of `name → DiscoveredApp`. Cheap to
/// clone — the inner is an `Arc<Mutex<...>>`.
#[derive(Clone, Default)]
pub struct DiscoveryTable {
    inner: Arc<Mutex<HashMap<String, DiscoveredApp>>>,
}

impl DiscoveryTable {
    /// Look up a service by name. Returns a snapshot — safe to use
    /// even if the entry vanishes mid-call.
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

/// Start a background thread that periodically scans
/// `~/.idealyst/apps/` for live registration files and keeps the
/// [`DiscoveryTable`] up to date. Returns the table immediately;
/// the thread runs for the lifetime of the process.
///
/// If the home dir or apps directory doesn't exist yet the scanner
/// just keeps polling — apps that launch later get picked up on
/// the next pass.
pub fn start() -> DiscoveryTable {
    let table = DiscoveryTable::default();
    let table_for_thread = table.clone();

    std::thread::Builder::new()
        .name("idealyst-apps-scanner".into())
        .spawn(move || run_scanner(table_for_thread))
        .ok();

    table
}

/// 1s scan cadence: fast enough that newly-launched apps show up
/// in the MCP server's `list_apps` within one tick, cheap enough to
/// not matter — each scan is a `readdir` + small-JSON parse per
/// entry.
const SCAN_INTERVAL: Duration = Duration::from_secs(1);

fn run_scanner(table: DiscoveryTable) {
    loop {
        if let Some(dir) = apps_dir() {
            rescan_into(&dir, &table);
        }
        std::thread::sleep(SCAN_INTERVAL);
    }
}

fn rescan_into(dir: &Path, table: &DiscoveryTable) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut found: HashMap<String, DiscoveredApp> = HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(app) = parse_registration_file(&path) else {
            continue;
        };
        // Liveness check — drop stale files left by crashed processes
        // so the MCP server doesn't try to dial dead ports. ESRCH on
        // `kill(pid, 0)` means the process is gone; EPERM means it's
        // alive (just not ours to signal).
        if !pid_is_live(app.pid) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        found.insert(app.name.clone(), app);
    }
    if let Ok(mut guard) = table.inner.lock() {
        *guard = found;
    }
}

fn parse_registration_file(path: &Path) -> Option<DiscoveredApp> {
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let name = v.get("name")?.as_str()?.to_string();
    let pid = v.get("pid")?.as_u64()? as u32;
    let port = v.get("port")?.as_u64()? as u16;
    let bundle_id = v
        .get("bundle_id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let project_root = v
        .get("project_root")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let catalog_bin = v
        .get("catalog_bin")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    Some(DiscoveredApp {
        name,
        bundle_id,
        project_root,
        catalog_bin,
        pid,
        bridge_addr: format!("127.0.0.1:{port}"),
    })
}

/// `kill(pid, 0)` — succeeds if the process is alive (or alive but
/// not signalable by us → EPERM); fails with ESRCH when gone.
#[cfg(unix)]
fn pid_is_live(pid: u32) -> bool {
    // SAFETY: `kill` with sig 0 is a no-op signal check on POSIX.
    let rc = unsafe { libc::kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }
    let err = std::io::Error::last_os_error();
    matches!(err.raw_os_error(), Some(libc::EPERM))
}

#[cfg(not(unix))]
fn pid_is_live(_pid: u32) -> bool {
    // Windows: punt for now; treat every registration as live and
    // rely on the bridge's RAII Drop to clean up on graceful exit.
    // Real liveness check would use `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)`
    // + `GetExitCodeProcess`.
    true
}

fn apps_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    Some(home.join(".idealyst").join("apps"))
}
