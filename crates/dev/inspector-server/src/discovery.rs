//! Find running apps by scanning the registration files their robot
//! bridges (or the `idealyst dev` relay they dialed) write.
//!
//! Each live app writes `~/.idealyst/apps/<name>-<pid>.json` containing
//! `{port, pid, name, bundle_id, project_root, platform, proto}` (see
//! `runtime_shared::robot::bridge` and `robot-relay`). The file stem is
//! the app's id on the Inspector protocol.

use std::path::{Path, PathBuf};

use inspector_protocol::AppInfo;

/// `~/.idealyst/apps`, where every bridge registers.
pub fn default_apps_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| PathBuf::from(h).join(".idealyst").join("apps"))
}

/// `true` if a process with `pid` exists. `kill -0` probes existence
/// without signalling, so there's no `libc` dependency.
#[cfg(unix)]
fn pid_is_live(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// No cheap probe without a platform dependency: every registration is
/// offered, and a dead one's port simply refuses the connection, which
/// the session reports.
#[cfg(not(unix))]
fn pid_is_live(_pid: u32) -> bool {
    true
}

/// Every registered, live app in `dir`, name-sorted. Empty if the
/// directory is missing or unreadable.
pub fn list(dir: &Path) -> Vec<AppInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
            continue;
        };
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
        if !pid_is_live(pid) {
            // A crashed or killed app's registration (its cleanup never
            // ran). Its port may since belong to an UNRELATED process
            // (`adb` squats on the default 9718) that accepts the socket
            // but never speaks the robot protocol.
            continue;
        }
        out.push(AppInfo {
            id,
            name: name.to_string(),
            bundle_id: v["bundle_id"].as_str().map(str::to_string),
            port: port as u16,
            pid,
            platform: v["platform"].as_str().map(str::to_string),
            project_root: v["project_root"].as_str().map(str::to_string),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_live_registrations_and_skips_dead_and_malformed_ones() {
        let dir = tempfile::tempdir().unwrap();
        let me = std::process::id();
        std::fs::write(
            dir.path().join(format!("Todo-{me}.json")),
            format!(r#"{{"port":5001,"pid":{me},"name":"Todo","platform":"web","proto":1}}"#),
        )
        .unwrap();
        // No process has pid u32::MAX - 1.
        std::fs::write(dir.path().join("Gone-1.json"), r#"{"port":5002,"pid":4294967294,"name":"Gone"}"#).unwrap();
        std::fs::write(dir.path().join("junk.json"), "not json").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "{}").unwrap();

        let apps = list(dir.path());
        #[cfg(unix)]
        assert_eq!(apps.len(), 1, "{apps:?}");
        let todo = apps.iter().find(|a| a.name == "Todo").expect("Todo listed");
        assert_eq!(todo.id, format!("Todo-{me}"));
        assert_eq!(todo.addr(), "127.0.0.1:5001");
        assert_eq!(todo.platform.as_deref(), Some("web"));
    }

    #[test]
    fn missing_dir_is_empty() {
        assert!(list(Path::new("/definitely/not/here")).is_empty());
    }
}
