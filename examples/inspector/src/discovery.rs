//! App discovery — find running idealyst apps by scanning the per-process
//! registration files their robot bridge writes on bind.
//!
//! Each live app (built `--features robot`) writes
//! `~/.idealyst/apps/<name>-<pid>.json` containing `{port, pid, name,
//! bundle_id, project_root, proto}` (see `runtime_core::robot::bridge`).
//! We read every `*.json` there. Liveness isn't probed here — a dead app's
//! port simply won't accept a connection, which the client surfaces.

use std::path::PathBuf;

/// One discovered app the inspector can connect to.
#[derive(Clone, Debug)]
pub struct AppInfo {
    pub name: String,
    pub bundle_id: Option<String>,
    pub port: u16,
    pub pid: u32,
}

impl AppInfo {
    pub fn addr(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

fn apps_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| PathBuf::from(h).join(".idealyst").join("apps"))
}

/// List every registered app, name-sorted — EXCLUDING the inspector's own
/// process. Empty if the directory is missing or unreadable.
///
/// Self-exclusion is essential: the inspector also runs under `idealyst
/// dev` (which enables `runtime-core/dev` → `robot`), so its own bridge
/// registers a file here. Connecting the inspector to *itself* would make
/// its background poll introspect its own live, rendering reactive arena
/// on the same thread — a guaranteed re-entrant `RefCell` borrow panic.
/// Inspecting a *different* process is safe: all introspection runs over
/// there; the inspector only does TCP + its own `snapshot.set`.
pub fn list() -> Vec<AppInfo> {
    let Some(dir) = apps_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let self_pid = std::process::id();
    let mut out = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
            continue;
        };
        let (Some(port), Some(name)) = (v["port"].as_u64(), v["name"].as_str()) else {
            continue;
        };
        let pid = v["pid"].as_u64().unwrap_or(0) as u32;
        if pid == self_pid {
            continue; // never offer to connect to ourselves
        }
        out.push(AppInfo {
            name: name.to_string(),
            bundle_id: v["bundle_id"].as_str().map(|s| s.to_string()),
            port: port as u16,
            pid,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}
