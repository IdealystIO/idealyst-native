//! User-level app registry — the cross-project coordination point
//! the MCP server uses to enumerate running idealyst apps.
//!
//! Layout:
//!
//! ```text
//! ~/.idealyst/registry.json
//! {
//!   "apps": [
//!     {
//!       "name": "welcome",
//!       "project_root": "/path/to/welcome",
//!       "bridge_addr": "127.0.0.1:53891",
//!       "catalog_bin": "/path/.../welcome-macos",
//!       "pid": 12345,
//!       "registered_at": 1758569483
//!     },
//!     ...
//!   ]
//! }
//! ```
//!
//! Writers (`idealyst dev` + the framework's bridge auto-start) call
//! [`Registry::register`] when an app comes up and
//! [`Registry::deregister_pid`] on graceful exit. The MCP server (via
//! `idealyst mcp`) calls [`Registry::load`] on every Robot tool call
//! to find the addressed app.
//!
//! Concurrency: writes go through a tempfile + atomic rename so
//! concurrent registrations from two `idealyst dev` invocations can't
//! corrupt the file. Reads are tolerant of partial / missing files —
//! a fresh empty registry is returned.
//!
//! Liveness: stale entries (PID gone, bridge unreachable) survive
//! across reads. They're cleaned up by the next *register-with-same-
//! project-root* call (which replaces the old entry) or by an
//! explicit `idealyst mcp prune` command. We don't auto-probe TCP on
//! every read because it'd add up to ~hundreds of milliseconds in
//! a multi-app session — cheap-but-not-free.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Directory holding the registry, relative to `$HOME`.
pub const REGISTRY_DIR: &str = ".idealyst";

/// Filename within `REGISTRY_DIR`.
pub const REGISTRY_FILE: &str = "registry.json";

/// One running idealyst app. Identity is `(project_root, pid)`:
/// re-launching the same project from `idealyst dev` replaces the
/// old entry for that project_root rather than accumulating.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEntry {
    /// Human-readable app name, surfaced in tool errors + `list_apps`.
    /// Defaults to the `[package].name` of the user crate.
    pub name: String,
    /// Absolute path to the project directory (the dir containing
    /// `Cargo.toml`). Doubles as the identity key for re-registration.
    pub project_root: String,
    /// Robot bridge TCP address — what `RobotBridge::new(addr)` takes.
    pub bridge_addr: String,
    /// Path to a binary the MCP server can spawn with `--emit-catalog`
    /// to fetch the project's `framework_mcp::catalog_json()`. May
    /// be `None` for projects whose platform target doesn't (yet)
    /// support the mode (iOS / Android / web).
    #[serde(default)]
    pub catalog_bin: Option<String>,
    /// OS PID of the running app process. Used both for register-
    /// replacement and for future liveness probes.
    pub pid: u32,
    /// Seconds-since-epoch at registration time. Surfaces in
    /// `list_apps` so the user can see "registered 12 seconds ago".
    pub registered_at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub apps: Vec<AppEntry>,
}

/// Where `~/.idealyst/registry.json` lives. `$HOME` is required;
/// callers running without `HOME` set get a path under cwd as a
/// fallback (tests, CI shells, etc.). Not security-sensitive — the
/// registry is a coordination breadcrumb, not a secret store.
pub fn registry_path() -> PathBuf {
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(REGISTRY_DIR).join(REGISTRY_FILE)
}

impl Registry {
    /// Read the on-disk registry. Missing or malformed → empty.
    pub fn load() -> Self {
        let path = registry_path();
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => return Self::default(),
        };
        match serde_json::from_str::<Registry>(&raw) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "[registry] {} is not valid JSON ({}); ignoring",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    /// Atomically write the registry to disk. Uses a tempfile +
    /// rename so concurrent writers from two `idealyst dev`
    /// invocations can't tear the file mid-write.
    pub fn save(&self) -> std::io::Result<()> {
        let path = registry_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(self).unwrap();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Add `entry`, replacing any existing entry for the same
    /// `project_root` (so re-running `idealyst dev` against the same
    /// project doesn't leave a stale duplicate).
    pub fn register(&mut self, entry: AppEntry) {
        self.apps
            .retain(|e| e.project_root != entry.project_root);
        self.apps.push(entry);
    }

    /// Remove the entry for `project_root`. Idempotent — no-op if no
    /// matching entry. `idealyst dev`'s SIGINT handler calls this.
    pub fn deregister_project(&mut self, project_root: &Path) {
        let target = project_root.to_string_lossy();
        self.apps.retain(|e| e.project_root != target);
    }

    /// Remove the entry with the given PID. Used by graceful-exit
    /// paths where the dev launcher knows its own PID.
    pub fn deregister_pid(&mut self, pid: u32) {
        self.apps.retain(|e| e.pid != pid);
    }

    /// First app whose `name` exactly matches `app_name`. Case-
    /// sensitive — registry names mirror cargo package names.
    pub fn find(&self, app_name: &str) -> Option<&AppEntry> {
        self.apps.iter().find(|e| e.name == app_name)
    }
}

/// Convenience helper: load, mutate via `f`, save. Single-step for
/// callers that just want "add my entry and persist."
pub fn update_with<F: FnOnce(&mut Registry)>(f: F) -> std::io::Result<()> {
    let mut reg = Registry::load();
    f(&mut reg);
    reg.save()
}

/// Seconds since the UNIX epoch. `0` if the system clock is somehow
/// before epoch (shouldn't happen).
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_replaces_same_project() {
        let mut reg = Registry::default();
        reg.register(AppEntry {
            name: "a".into(),
            project_root: "/p".into(),
            bridge_addr: "127.0.0.1:9000".into(),
            catalog_bin: None,
            pid: 1,
            registered_at: 0,
        });
        reg.register(AppEntry {
            name: "a".into(),
            project_root: "/p".into(),
            bridge_addr: "127.0.0.1:9001".into(),
            catalog_bin: None,
            pid: 2,
            registered_at: 1,
        });
        assert_eq!(reg.apps.len(), 1);
        assert_eq!(reg.apps[0].pid, 2);
        assert_eq!(reg.apps[0].bridge_addr, "127.0.0.1:9001");
    }

    #[test]
    fn find_matches_by_name() {
        let mut reg = Registry::default();
        reg.register(AppEntry {
            name: "welcome".into(),
            project_root: "/p1".into(),
            bridge_addr: "127.0.0.1:1".into(),
            catalog_bin: None,
            pid: 1,
            registered_at: 0,
        });
        reg.register(AppEntry {
            name: "other".into(),
            project_root: "/p2".into(),
            bridge_addr: "127.0.0.1:2".into(),
            catalog_bin: None,
            pid: 2,
            registered_at: 0,
        });
        assert_eq!(reg.find("welcome").unwrap().pid, 1);
        assert_eq!(reg.find("other").unwrap().pid, 2);
        assert!(reg.find("missing").is_none());
    }

    #[test]
    fn deregister_is_idempotent() {
        let mut reg = Registry::default();
        reg.register(AppEntry {
            name: "a".into(),
            project_root: "/p".into(),
            bridge_addr: "127.0.0.1:1".into(),
            catalog_bin: None,
            pid: 1,
            registered_at: 0,
        });
        let p = PathBuf::from("/p");
        reg.deregister_project(&p);
        reg.deregister_project(&p); // no-op
        assert!(reg.apps.is_empty());
    }
}
