//! Filesystem-based discovery of running Idealyst apps.
//!
//! The Robot bridge inside each running app writes
//! `~/.idealyst/apps/<name>-<pid>.json` on bind (and removes it via
//! RAII on graceful shutdown). The MCP server scans that directory
//! to populate an in-memory map of currently-live apps keyed by app
//! name.
//!
//! Files rather than mDNS: no multicast firewall headaches on
//! corporate / VPN networks, and deterministic results across rerun
//! cycles.
//!
//! Liveness check: each scan does `kill(pid, 0)` (a no-op syscall
//! that fails with ESRCH when the process is gone) to filter ghost
//! entries that a crash left behind without RAII running. Stale files
//! are deleted at scan time.
//!
//! ## Project-local registrations (devcontainers)
//!
//! `idealyst dev` with a pinned relay port (`--robot-port` / `robot_port`
//! in `dev.toml`) also writes `<project>/.idealyst/robot.json`. A dev
//! session inside a devcontainer registers in the CONTAINER's
//! `~/.idealyst/apps`, which an MCP server on the host never sees — but
//! the project directory is shared, and the pinned port is forwarded to
//! the same port on the host. So each scan also reads `robot.json` in the
//! working directory and its ancestors. Its `pid` is the container's and
//! means nothing here: liveness is a TCP connect to its port instead, and
//! the file is never deleted (the relay removes it; a stale one is just
//! not live). An entry whose port an `~/.idealyst/apps` entry already
//! names is the same relay seen from inside, and is skipped.
//!
//! ## Wine registrations
//!
//! A Windows app running under Wine registers in `~/.idealyst/apps-wine`
//! instead (`run-windows` sets `IDEALYST_APPS_DIR`): its pid is a Wine pid,
//! not a host one, so the two kinds cannot share a directory pruned by pid.
//! Each scan reads that directory too, with the same rule as `robot.json`:
//! liveness is a TCP connect to the port, and the file is never deleted here
//! (the bridge inside Wine prunes its own directory by Wine pid).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One live app as discovered via the per-process registration file.
#[derive(Debug, Clone)]
pub struct DiscoveredApp {
    /// `name` field from the JSON — the
    /// `runtime_shared::robot::bridge::AppIdentity::name`.
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
    /// Lowercase platform tag (`web`, `macos`, `ios`, `android`, …) from the
    /// registration file. `None` for older apps / a relay that hadn't yet
    /// seen the app's `hello`. Lets a parity caller target two platforms of
    /// the **same** app distinctly.
    pub platform: Option<String>,
}

/// Live, lock-protected map of `name → DiscoveredApp`. Cheap to
/// clone — the inner is an `Arc<Mutex<...>>`.
#[derive(Clone, Default)]
pub struct DiscoveryTable {
    inner: Arc<Mutex<HashMap<String, DiscoveredApp>>>,
}

impl DiscoveryTable {
    /// Look up a service by name. Returns the first match (the map is keyed
    /// by `name#pid` so two platforms of the same app coexist; this returns
    /// whichever the iteration finds first — callers that must disambiguate
    /// use [`snapshot`](Self::snapshot) + `platform`).
    pub fn get(&self, name: &str) -> Option<DiscoveredApp> {
        self.inner.lock().ok()?.values().find(|a| a.name == name).cloned()
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
    // Fixed at start, like the process's own working directory.
    let cwd = std::env::current_dir().ok();
    loop {
        let mut found = apps_dir().map(|dir| scan_apps_dir(&dir)).unwrap_or_default();
        if let Some(dir) = apps_dir().map(|d| d.with_file_name(WINE_APPS_DIR_NAME)) {
            add_wine_registrations(&dir, &mut found, relay_is_live);
        }
        if let Some(cwd) = &cwd {
            add_project_registration(cwd, &mut found, relay_is_live);
        }
        if let Ok(mut guard) = table.inner.lock() {
            *guard = found;
        }
        std::thread::sleep(SCAN_INTERVAL);
    }
}

/// The project-local registration's path, inside `.idealyst/`.
pub const PROJECT_REGISTRATION: &str = "robot.json";

/// Add the nearest `<dir>/.idealyst/robot.json` at or above `cwd` — see
/// the module docs — when `live` says its port answers and no entry in
/// `found` already names that port.
fn add_project_registration(
    cwd: &Path,
    found: &mut HashMap<String, DiscoveredApp>,
    live: impl Fn(&str) -> bool,
) {
    let Some(path) = cwd
        .ancestors()
        .map(|d| d.join(".idealyst").join(PROJECT_REGISTRATION))
        .find(|p| p.is_file())
    else {
        return;
    };
    let Some(app) = parse_registration_file(&path) else {
        return;
    };
    if found.values().any(|a| a.bridge_addr == app.bridge_addr) || !live(&app.bridge_addr) {
        return;
    }
    found.insert(format!("{}@{}", app.name, app.bridge_addr), app);
}

/// Registry directory, beside `~/.idealyst/apps`, that Wine apps register
/// in — the same name `robot_test::WINE_APPS_DIR_NAME` and `run-windows` use.
const WINE_APPS_DIR_NAME: &str = "apps-wine";

/// Add every registration in the Wine registry `dir` whose port `live` says
/// answers (see "Wine registrations" in the module docs). Never deletes: a
/// Wine pid can't be checked from the host, so a dead entry is just skipped.
fn add_wine_registrations(
    dir: &Path,
    found: &mut HashMap<String, DiscoveredApp>,
    live: impl Fn(&str) -> bool,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(app) = parse_registration_file(&path) else {
            continue;
        };
        if found.values().any(|a| a.bridge_addr == app.bridge_addr) || !live(&app.bridge_addr) {
            continue;
        }
        found.insert(format!("{}#wine{}", app.name, app.pid), app);
    }
}

/// Whether something accepts connections at `addr`.
fn relay_is_live(addr: &str) -> bool {
    addr.parse::<std::net::SocketAddr>().is_ok_and(|a| {
        std::net::TcpStream::connect_timeout(&a, Duration::from_millis(200)).is_ok()
    })
}

fn scan_apps_dir(dir: &Path) -> HashMap<String, DiscoveredApp> {
    let mut found: HashMap<String, DiscoveredApp> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
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
        // Key by name+pid (not name alone) so two platforms of the SAME app —
        // identical name, different process — both survive instead of one
        // clobbering the other. Parity work needs to see both.
        found.insert(format!("{}#{}", app.name, app.pid), app);
    }
    found
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
    let platform = v
        .get("platform")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty() && *s != "unknown")
        .map(|s| s.to_string());
    Some(DiscoveredApp {
        name,
        bundle_id,
        project_root,
        catalog_bin,
        pid,
        bridge_addr: format!("127.0.0.1:{port}"),
        platform,
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

/// Why `~/.idealyst/apps` might hold no registration, for a `wait_for_app`
/// that found nothing: missing (nothing registered yet — or nothing could),
/// not writable (no app on this machine can register), or present and
/// empty. Read as this process; the dev session writing registrations runs
/// as the same user, so what this process can write, it can.
pub(crate) fn describe_apps_dir() -> String {
    describe_apps_dir_at(apps_dir())
}

fn describe_apps_dir_at(dir: Option<PathBuf>) -> String {
    let Some(dir) = dir else {
        return "there is no HOME, so no ~/.idealyst/apps to register in".to_string();
    };
    if dir.is_dir() {
        return if writable(&dir) {
            format!("{} holds no live registration", dir.display())
        } else {
            format!("{} is not writable by this user, so no app can register there", dir.display())
        };
    }
    // Missing: fine if it can still be created; a dead end if its nearest
    // existing ancestor can't be written (e.g. a root-owned ~/.idealyst).
    let mut ancestor = dir.parent();
    while let Some(a) = ancestor {
        if a.exists() {
            if !a.is_dir() || !writable(a) {
                return format!(
                    "{} does not exist and can't be created ({} is not a writable directory), \
                     so no app can register there",
                    dir.display(),
                    a.display()
                );
            }
            break;
        }
        ancestor = a.parent();
    }
    format!("{} does not exist — nothing has registered on this machine", dir.display())
}

#[cfg(unix)]
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: `c` is a valid NUL-terminated path for the call's duration.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(not(unix))]
fn writable(path: &Path) -> bool {
    !std::fs::metadata(path).map(|m| m.permissions().readonly()).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_dir_description_names_each_state() {
        let root = std::env::temp_dir().join(format!("apps_dir_note_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let missing = root.join("a").join(".idealyst").join("apps");
        assert!(describe_apps_dir_at(Some(missing.clone())).contains("does not exist — nothing has registered"));

        let present = root.join("b").join("apps");
        std::fs::create_dir_all(&present).unwrap();
        assert!(describe_apps_dir_at(Some(present)).contains("holds no live registration"));

        // `.idealyst` is a file: the apps dir can never be created.
        std::fs::create_dir_all(root.join("c")).unwrap();
        std::fs::write(root.join("c").join(".idealyst"), "").unwrap();
        let blocked = root.join("c").join(".idealyst").join("apps");
        assert!(describe_apps_dir_at(Some(blocked)).contains("can't be created"));

        assert!(describe_apps_dir_at(None).contains("no HOME"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The bench failure that motivated the note: a root-owned
    /// `~/.idealyst` (a bind mount under it made Docker create it) left the
    /// apps dir uncreatable, and `wait_for_app` blamed the build.
    #[cfg(unix)]
    #[test]
    fn regression_an_unwritable_parent_is_named() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root ignores permission bits
        }
        let root = std::env::temp_dir().join(format!("apps_dir_ro_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let idealyst = root.join(".idealyst");
        std::fs::create_dir_all(&idealyst).unwrap();
        std::fs::set_permissions(&idealyst, std::fs::Permissions::from_mode(0o555)).unwrap();
        let note = describe_apps_dir_at(Some(idealyst.join("apps")));
        std::fs::set_permissions(&idealyst, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(note.contains("can't be created") && note.contains(".idealyst"), "{note}");
    }

    fn write_registration(dir: &Path, port: u16) {
        std::fs::create_dir_all(dir.join(".idealyst")).unwrap();
        std::fs::write(
            dir.join(".idealyst").join(PROJECT_REGISTRATION),
            format!(r#"{{"port":{port},"pid":12,"name":"todo","bundle_id":null,"project_root":"/workspaces/todo","proto":1}}"#),
        )
        .unwrap();
    }

    /// An MCP server on a devcontainer's host finds the container's dev
    /// relay through the project directory — from the project or any
    /// directory under it — when its (forwarded) port answers; the
    /// container's pid is not checked.
    #[test]
    fn a_project_registration_is_found_from_below_when_its_port_answers() {
        let root = std::env::temp_dir().join(format!("mcp-project-reg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write_registration(&root, 4778);
        let below = root.join("crates").join("app");
        std::fs::create_dir_all(&below).unwrap();

        let mut found = HashMap::new();
        add_project_registration(&below, &mut found, |addr| addr == "127.0.0.1:4778");
        let app = found.values().next().expect("found");
        assert_eq!(app.name, "todo");
        assert_eq!(app.bridge_addr, "127.0.0.1:4778");

        let mut dead = HashMap::new();
        add_project_registration(&below, &mut dead, |_| false);
        assert!(dead.is_empty(), "a port that does not answer is not live");
        assert!(root.join(".idealyst").join(PROJECT_REGISTRATION).is_file(), "and the file is left alone");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Regression: Wine apps register in `~/.idealyst/apps-wine`, which the
    /// MCP server never read — a Windows app under Wine was invisible to
    /// `list_apps`. Its pid is a Wine pid, so liveness is the port, and a
    /// dead entry is skipped but never deleted.
    #[test]
    fn regression_wine_registrations_are_listed_by_port_and_never_pruned() {
        let dir = std::env::temp_dir().join(format!("mcp-wine-reg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (file, port) in [("live-1.json", 4790), ("dead-2.json", 4791)] {
            std::fs::write(
                dir.join(file),
                format!(r#"{{"port":{port},"pid":{port},"name":"winapp","bundle_id":null,"project_root":"","platform":"windows","proto":1}}"#),
            )
            .unwrap();
        }
        let mut found = HashMap::new();
        add_wine_registrations(&dir, &mut found, |addr| addr == "127.0.0.1:4790");
        assert_eq!(found.len(), 1, "only the answering port is live: {found:?}");
        let app = found.values().next().unwrap();
        assert_eq!((app.name.as_str(), app.bridge_addr.as_str()), ("winapp", "127.0.0.1:4790"));
        assert!(dir.join("dead-2.json").is_file(), "a Wine registration is never deleted from the host");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Inside the container the same relay is also in `~/.idealyst/apps`:
    /// listed once.
    #[test]
    fn a_project_registration_already_in_the_apps_dir_is_listed_once() {
        let root = std::env::temp_dir().join(format!("mcp-project-reg-dup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write_registration(&root, 4779);
        let mut found = HashMap::new();
        found.insert(
            "todo#12".to_string(),
            DiscoveredApp {
                name: "todo".into(),
                bundle_id: None,
                project_root: None,
                catalog_bin: None,
                pid: 12,
                bridge_addr: "127.0.0.1:4779".into(),
                platform: Some("web".into()),
            },
        );
        add_project_registration(&root, &mut found, |_| true);
        assert_eq!(found.len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
