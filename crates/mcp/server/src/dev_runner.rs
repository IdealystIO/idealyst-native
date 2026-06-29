//! Spawn + track `idealyst dev` sessions from the MCP server.
//!
//! The CLI's `idealyst dev` orchestrates a whole platform fleet (web /
//! iOS / Android / macOS / terminal) in ONE long-lived foreground
//! process that tears every child down on Ctrl-C. An MCP client can't
//! hold a foreground process, so this module spawns `idealyst dev`
//! detached — stdout/stderr redirected to a per-session log file under
//! `~/.idealyst/dev-logs/` — records the child, and exposes a `stop`
//! that signals the whole session back down.
//!
//! ## Why a fresh process group (unix)
//!
//! `idealyst dev` itself spawns cargo builds, web static servers, and
//! simulator launchers as its own children. Killing just the `dev` pid
//! would orphan that subtree. So on unix we put the child in a NEW
//! process group (`process_group(0)` → pgid == child pid) and signal
//! the whole group with `kill(-pgid, …)`. We send `SIGINT` first so the
//! `dev` process's own Ctrl-C handler runs its graceful teardown
//! (killing simulators / servers cleanly), then escalate to `SIGKILL`
//! for anything that didn't exit. Non-unix falls back to `Child::kill`,
//! which only reaps the `dev` pid (best effort).
//!
//! The MCP server binary IS the `idealyst` binary (`idealyst mcp`), so
//! `std::env::current_exe()` is exactly the CLI to re-invoke with `dev`.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// What a caller wants launched. Mirrors the subset of `idealyst dev`
/// flags that make sense to drive from MCP: target platforms, the
/// `--local` mode toggle, and the robot knobs.
#[derive(Debug, Clone, Default)]
pub struct DevLaunch {
    /// Project directory (the `dir` positional). `None` → the MCP
    /// server's current working directory (the project it was started
    /// in), matching the bare `idealyst dev` behavior.
    pub dir: Option<PathBuf>,
    /// Target platforms — any of `web`, `ios`, `android`, `macos`,
    /// `terminal`. Empty + `all == false` → `idealyst dev` falls back
    /// to the manifest's declared `targets`.
    pub platforms: Vec<String>,
    /// `--all`: every platform the host can build for.
    pub all: bool,
    /// `--local`: build the app natively per platform (no runtime-server
    /// wire). See the CLI docs for the trade-off.
    pub local: bool,
    /// `--no-robot`: disable the Robot bridge/relay for this session.
    pub no_robot: bool,
    /// `--bridge-port`: pin the Robot bridge to a fixed port.
    pub bridge_port: Option<u16>,
    /// `--screenshot-dir`: where the relay saves screenshot PNGs.
    pub screenshot_dir: Option<PathBuf>,
    /// `--no-build`: web only — skip the initial build.
    pub no_build: bool,
}

/// A live (or recently-exited) `idealyst dev` session the server is
/// tracking.
struct DevSession {
    id: u64,
    child: Child,
    /// Process-group id to signal on stop. On unix this is the child's
    /// pid (it leads a fresh group); unused on other platforms.
    #[allow(dead_code)]
    pgid: u32,
    dir: PathBuf,
    platforms: Vec<String>,
    local: bool,
    no_robot: bool,
    log_path: PathBuf,
    started_at: SystemTime,
    /// Set once we've observed the child exit (so `list` reports it and
    /// the next prune drops it).
    exit_status: Option<i32>,
}

/// A JSON-friendly view of one session for tool responses.
#[derive(Debug, serde::Serialize)]
pub struct DevSessionInfo {
    pub id: u64,
    pub pid: u32,
    pub dir: String,
    pub platforms: Vec<String>,
    pub local: bool,
    pub no_robot: bool,
    pub log_path: String,
    /// Seconds since launch.
    pub uptime_secs: u64,
    /// `running`, or `exited(<code>)` once the process has finished.
    pub status: String,
}

impl DevSession {
    fn info(&self) -> DevSessionInfo {
        let status = match self.exit_status {
            None => "running".to_string(),
            Some(code) => format!("exited({code})"),
        };
        DevSessionInfo {
            id: self.id,
            pid: self.child.id(),
            dir: self.dir.to_string_lossy().into_owned(),
            platforms: self.platforms.clone(),
            local: self.local,
            no_robot: self.no_robot,
            log_path: self.log_path.to_string_lossy().into_owned(),
            uptime_secs: self
                .started_at
                .elapsed()
                .map(|d| d.as_secs())
                .unwrap_or(0),
            status,
        }
    }
}

/// Tracks every `idealyst dev` session this server launched. Cloned
/// into the [`crate::CatalogService`] via `Arc`; the inner `Mutex`
/// guards the session table.
#[derive(Default)]
pub struct DevRunner {
    sessions: Mutex<Vec<DevSession>>,
    next_id: AtomicU64,
}

impl DevRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Launch `idealyst dev` for `launch`, detached, with stdio routed
    /// to a per-session log file. Returns the new session's info.
    pub fn start(&self, launch: DevLaunch) -> Result<DevSessionInfo, String> {
        // The CLI to re-invoke. `idealyst mcp` runs from the `idealyst`
        // binary, so current_exe IS the CLI — no PATH lookup needed, and
        // it can't accidentally pick a different `idealyst` build.
        let exe = std::env::current_exe()
            .map_err(|e| format!("cannot resolve current executable: {e}"))?;

        let dir = match &launch.dir {
            Some(d) => d.clone(),
            None => std::env::current_dir()
                .map_err(|e| format!("cannot resolve current directory: {e}"))?,
        };
        if !dir.is_dir() {
            return Err(format!("project dir does not exist: {}", dir.display()));
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let app_name = dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "app".to_string());
        let log_path = dev_log_path(&app_name, id)?;

        let log = std::fs::File::create(&log_path)
            .map_err(|e| format!("cannot create dev log {}: {e}", log_path.display()))?;
        let log_err = log
            .try_clone()
            .map_err(|e| format!("cannot dup dev log handle: {e}"))?;

        let mut cmd = Command::new(&exe);
        cmd.arg("dev").arg(&dir);
        validate_platforms(&launch.platforms)?;
        for p in &launch.platforms {
            cmd.arg(format!("--{}", p.to_lowercase()));
        }
        if launch.all {
            cmd.arg("--all");
        }
        if launch.local {
            cmd.arg("--local");
        }
        if launch.no_robot {
            cmd.arg("--no-robot");
        }
        if let Some(port) = launch.bridge_port {
            cmd.arg("--bridge-port").arg(port.to_string());
        }
        if let Some(sdir) = &launch.screenshot_dir {
            cmd.arg("--screenshot-dir").arg(sdir);
        }
        if launch.no_build {
            cmd.arg("--no-build");
        }

        self.spawn_tracked(cmd, id, dir, launch, log_path, log, log_err)
    }

    /// Finish a launch: wire up stdio/process-group, spawn, and record
    /// the session. Split out from [`start`] so tests can drive the
    /// track/stop machinery with a controllable child command instead of
    /// the real `idealyst dev` binary.
    fn spawn_tracked(
        &self,
        mut cmd: Command,
        id: u64,
        dir: PathBuf,
        launch: DevLaunch,
        log_path: PathBuf,
        log: std::fs::File,
        log_err: std::fs::File,
    ) -> Result<DevSessionInfo, String> {
        // Detach: no stdin (so the dev TUI / prompts never block on a
        // tty that isn't there), stdout+stderr to the log file.
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::from(log));
        cmd.stderr(Stdio::from(log_err));

        // Fresh process group so `stop` can signal the whole dev subtree
        // (cargo builds, web servers, simulators) at once.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }

        let child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn `idealyst dev`: {e}"))?;
        let pgid = child.id();

        let session = DevSession {
            id,
            child,
            pgid,
            dir,
            platforms: launch.platforms,
            local: launch.local,
            no_robot: launch.no_robot,
            log_path,
            started_at: SystemTime::now(),
            exit_status: None,
        };
        let info = session.info();
        self.sessions.lock().unwrap().push(session);
        Ok(info)
    }

    /// Snapshot of every tracked session. Reaps any that have exited
    /// (updating their status) but keeps exited entries in the list so
    /// the caller sees the final state once; a subsequent call prunes
    /// the ones already reported exited.
    pub fn list(&self) -> Vec<DevSessionInfo> {
        let mut guard = self.sessions.lock().unwrap();
        // Drop sessions that were already reported as exited on a prior
        // call (status was Some before this reap) — one-shot reporting.
        guard.retain(|s| s.exit_status.is_none());
        for s in guard.iter_mut() {
            if let Ok(Some(status)) = s.child.try_wait() {
                s.exit_status = Some(status.code().unwrap_or(-1));
            }
        }
        guard.iter().map(|s| s.info()).collect()
    }

    /// Resolve the log file for a session id. Checks the live table
    /// first; if the session was already pruned (reported exited on a
    /// prior `list`), falls back to globbing `~/.idealyst/dev-logs/` for
    /// `*-<id>.log` — the file outlives the tracked session, so logs of a
    /// finished session stay readable.
    pub fn log_path(&self, id: u64) -> Option<PathBuf> {
        if let Some(p) = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.log_path.clone())
        {
            return Some(p);
        }
        let dir = dev_logs_dir().ok()?;
        let suffix = format!("-{id}.log");
        std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().ends_with(&suffix))
                    .unwrap_or(false)
            })
    }

    /// Stop one session by id. Graceful SIGINT → escalate to SIGKILL.
    /// Removes it from the table. Returns its final info.
    pub fn stop(&self, id: u64) -> Result<DevSessionInfo, String> {
        let mut guard = self.sessions.lock().unwrap();
        let idx = guard
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| format!("no dev session with id {id}"))?;
        let mut session = guard.remove(idx);
        drop(guard);
        terminate(&mut session);
        Ok(session.info())
    }

    /// Stop every tracked session. Returns each one's final info.
    pub fn stop_all(&self) -> Vec<DevSessionInfo> {
        let mut sessions = {
            let mut guard = self.sessions.lock().unwrap();
            std::mem::take(&mut *guard)
        };
        for s in sessions.iter_mut() {
            terminate(s);
        }
        sessions.iter().map(|s| s.info()).collect()
    }
}

impl Drop for DevRunner {
    /// Best-effort cleanup: if the MCP server shuts down normally (the
    /// client disconnected and `service.waiting()` returned), tear down
    /// any dev sessions we still own rather than orphaning them. A
    /// signal-killed server won't run this, but the explicit `stop_dev`
    /// tool and the user's own Ctrl-C cover that path.
    fn drop(&mut self) {
        if let Ok(mut guard) = self.sessions.lock() {
            for s in guard.iter_mut() {
                terminate(s);
            }
        }
    }
}

/// Signal a session's process (group) down: graceful first, hard if it
/// lingers. Records the observed exit code on the session.
fn terminate(session: &mut DevSession) {
    if session.exit_status.is_some() {
        return;
    }

    #[cfg(unix)]
    {
        // Negative pid → the whole process group. SIGINT triggers the
        // dev process's Ctrl-C handler (clean simulator/server
        // teardown); fall through to SIGKILL for stragglers.
        let pgid = session.pgid as i32;
        unsafe {
            libc::kill(-pgid, libc::SIGINT);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match session.child.try_wait() {
                Ok(Some(status)) => {
                    session.exit_status = Some(status.code().unwrap_or(-1));
                    return;
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => break,
            }
        }
        // Still alive after the grace window — hard-kill the group and
        // reap, recording whatever exit status `wait` reports.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
        let code = session.child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
        session.exit_status = Some(code);
    }

    #[cfg(not(unix))]
    {
        // No process-group teardown available — kill the dev pid only.
        let _ = session.child.kill();
        let _ = session.child.wait();
        session.exit_status = Some(-1);
    }
}

/// Read the tail of a dev log: the last `lines` lines, optionally
/// keeping only lines that contain `filter` (case-insensitive). Filter
/// is applied BEFORE the tail, so `filter: "error"` yields the last N
/// *matching* lines rather than N lines that happen to mention an error.
pub fn tail_log(
    path: &std::path::Path,
    lines: usize,
    filter: Option<&str>,
) -> Result<String, String> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read dev log {}: {e}", path.display()))?;
    let needle = filter.map(|f| f.to_lowercase());
    let mut kept: Vec<&str> = body
        .lines()
        .filter(|l| match &needle {
            Some(n) => l.to_lowercase().contains(n.as_str()),
            None => true,
        })
        .collect();
    if kept.len() > lines {
        kept = kept.split_off(kept.len() - lines);
    }
    Ok(kept.join("\n"))
}

/// Reject unknown platform tags up front so the spawned `idealyst dev`
/// doesn't fail later with a clap error the caller never sees (its
/// stderr goes to the log file, not the tool response).
fn validate_platforms(platforms: &[String]) -> Result<(), String> {
    const VALID: &[&str] = &["web", "ios", "android", "macos", "terminal"];
    for p in platforms {
        if !VALID.contains(&p.to_lowercase().as_str()) {
            return Err(format!(
                "unknown platform {p:?}; valid: web, ios, android, macos, terminal"
            ));
        }
    }
    Ok(())
}

/// `~/.idealyst/dev-logs/`. Mirrors the `~/.idealyst/` convention the
/// app-discovery + screenshot paths already use. Creates the directory.
fn dev_logs_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| "cannot resolve home directory (HOME/USERPROFILE unset)".to_string())?;
    let dir = home.join(".idealyst").join("dev-logs");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create dev-logs dir {}: {e}", dir.display()))?;
    Ok(dir)
}

/// `~/.idealyst/dev-logs/<name>-<id>.log`.
fn dev_log_path(app_name: &str, id: u64) -> Result<PathBuf, String> {
    Ok(dev_logs_dir()?.join(format!("{app_name}-{id}.log")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_platform() {
        let err = validate_platforms(&["web".into(), "wat".into()]).unwrap_err();
        assert!(err.contains("wat"), "{err}");
        // Case-insensitive accept.
        validate_platforms(&["Web".into(), "IOS".into()]).unwrap();
    }

    #[test]
    fn tail_log_lines_and_filter() {
        let dir = std::env::temp_dir().join(format!("idealyst-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.log");
        std::fs::write(
            &path,
            "build start\nwarning: unused\ncompiling\nerror: boom\nERROR: again\ndone\n",
        )
        .unwrap();

        // Tail without filter: last 2 lines.
        assert_eq!(tail_log(&path, 2, None).unwrap(), "ERROR: again\ndone");

        // Filter (case-insensitive) THEN tail: both error lines survive.
        assert_eq!(
            tail_log(&path, 10, Some("error")).unwrap(),
            "error: boom\nERROR: again"
        );

        // Filter + tail cap: only the last matching line.
        assert_eq!(tail_log(&path, 1, Some("error")).unwrap(), "ERROR: again");

        // Missing file is an error, not a panic.
        assert!(tail_log(&dir.join("nope.log"), 5, None).is_err());
    }

    #[test]
    fn log_path_under_idealyst() {
        let p = dev_log_path("demo", 3).unwrap();
        assert!(p.ends_with("demo-3.log"), "{p:?}");
        assert!(p.to_string_lossy().contains(".idealyst"));
    }

    #[test]
    fn start_rejects_missing_dir() {
        let runner = DevRunner::new();
        let launch = DevLaunch {
            dir: Some(PathBuf::from("/no/such/dir/xyzzy")),
            ..Default::default()
        };
        let err = runner.start(launch).unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
    }

    #[test]
    fn log_path_globs_for_pruned_session() {
        // A session that isn't tracked (e.g. already exited + pruned) is
        // still resolvable by globbing the dev-logs dir for `*-<id>.log`.
        let runner = DevRunner::new();
        // Use a high id unlikely to collide with a real concurrent run.
        let id = 987_654_u64;
        let path = dev_log_path("pruned-demo", id).unwrap();
        std::fs::write(&path, "old logs\n").unwrap();
        let found = runner.log_path(id).expect("glob fallback finds the file");
        assert_eq!(found, path);
        let _ = std::fs::remove_file(&path);
        // Unknown id resolves to nothing.
        assert!(runner.log_path(123_456_789).is_none());
    }

    #[test]
    fn list_starts_empty() {
        let runner = DevRunner::new();
        assert!(runner.list().is_empty());
        assert!(runner.stop_all().is_empty());
        assert!(runner.stop(99).is_err());
    }

    /// Drive the real spawn/track/stop machinery with a long-lived
    /// `sleep` stand-in for `idealyst dev`: a session must show up as
    /// `running` in `list`, and `stop` must signal its process group
    /// down (graceful SIGINT path) and drop it from the table.
    ///
    /// Unix-only because the group-signal teardown — the behavior under
    /// test — is unix-specific (`process_group(0)` + `kill(-pgid, …)`);
    /// `start` wires `process_group` for the real child too.
    #[cfg(unix)]
    #[test]
    fn start_list_stop_lifecycle() {
        let runner = DevRunner::new();
        let id = runner.next_id.fetch_add(1, Ordering::Relaxed);
        let log_path = dev_log_path("lifecycle-test", id).unwrap();
        let log = std::fs::File::create(&log_path).unwrap();
        let log_err = log.try_clone().unwrap();

        // A controllable child that stays alive until we stop it.
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let info = runner
            .spawn_tracked(
                cmd,
                id,
                std::env::temp_dir(),
                DevLaunch::default(),
                log_path,
                log,
                log_err,
            )
            .unwrap();
        assert_eq!(info.status, "running");

        let listed = runner.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert_eq!(listed[0].status, "running");

        // Stop it: SIGINT the group → the sleep has no handler so it
        // dies on the signal well within the grace window.
        let stopped = runner.stop(id).unwrap();
        assert_eq!(stopped.id, id);
        // Table is now empty and the pid is reaped.
        assert!(runner.list().is_empty());
        assert!(runner.stop(id).is_err());
    }
}
