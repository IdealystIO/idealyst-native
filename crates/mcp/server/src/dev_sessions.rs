//! Every `idealyst dev` session running on this machine — one a person
//! started in a terminal as much as one `run_dev` started here — found
//! through the session registry (`dev_events::sessions`) and driven
//! through each session's control endpoint, so an agent works with a
//! person's session the way it works with its own.
//!
//! Stopping a session goes by its pid, not its endpoint: the case it is
//! for is a session too wedged to answer.

use std::time::{Duration, Instant};

use dev_events::sessions::{self, SessionEntry};

/// How long a control request may take before the session counts as not
/// answering.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A registered session whose process is alive.
#[derive(Debug, Clone)]
pub struct Live {
    pub entry: SessionEntry,
}

/// Every live registered session. An entry whose process is gone (the
/// session crashed, or was killed before it could remove it) is deleted
/// on the way.
pub fn live() -> Vec<Live> {
    let Some(dir) = sessions::registry_dir() else { return Vec::new() };
    live_in(&dir)
}

pub(crate) fn live_in(dir: &std::path::Path) -> Vec<Live> {
    sessions::read_all(dir)
        .into_iter()
        .filter_map(|(path, entry)| {
            if crate::app_discovery::pid_is_live(entry.pid) {
                Some(Live { entry })
            } else {
                let _ = std::fs::remove_file(&path);
                None
            }
        })
        .collect()
}

/// The live registered session of process `pid`.
pub fn find(pid: u32) -> Option<Live> {
    live().into_iter().find(|l| l.entry.pid == pid)
}

/// The live registered session running in project directory `dir`.
pub fn running_in(dir: &std::path::Path) -> Option<Live> {
    let live = live();
    let entries: Vec<SessionEntry> = live.iter().map(|l| l.entry.clone()).collect();
    let pid = sessions::in_dir(&entries, dir)?.pid;
    live.into_iter().find(|l| l.entry.pid == pid)
}

/// Whose a live session is, as this server sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// `run_dev` here started it; the id it gave it.
    ThisServer(u64),
    /// An `idealyst mcp` started it and has since exited (an update or a
    /// client restart replaces the server without stopping its
    /// sessions). Nothing else owns it, so this server treats it as its
    /// own.
    ExitedMcp,
    /// An `idealyst mcp` that is still running started it: another
    /// agent's.
    OtherMcp,
    /// A person started it in a terminal.
    Person,
}

impl Owner {
    /// `entry`'s owner. `this_server` is the id `run_dev` here gave it,
    /// if it did.
    pub fn of(entry: &SessionEntry, this_server: Option<u64>) -> Self {
        Self::with_parent(entry, this_server, parent_pid)
    }

    fn with_parent(entry: &SessionEntry, this_server: Option<u64>, parent: impl Fn(u32) -> Option<u32>) -> Self {
        if let Some(id) = this_server {
            return Owner::ThisServer(id);
        }
        if entry.launched_by != "mcp" {
            return Owner::Person;
        }
        let gone = match entry.launcher_pid {
            Some(launcher) => !crate::app_discovery::pid_is_live(launcher),
            // An entry from before `launcher_pid`: the session's parent is
            // the server that spawned it until that server exits, when the
            // session is re-parented to init. Unknown counts as running:
            // stopping another agent's session is the worse mistake.
            None => parent(entry.pid) == Some(1),
        };
        if gone {
            Owner::ExitedMcp
        } else {
            Owner::OtherMcp
        }
    }

    /// Whether a stop needs `include_external`.
    pub fn is_external(self) -> bool {
        matches!(self, Owner::OtherMcp | Owner::Person)
    }

    /// As `list_dev_sessions` shows it.
    pub fn label(self) -> &'static str {
        match self {
            Owner::ThisServer(_) => "this server",
            Owner::ExitedMcp => "an idealyst mcp that has exited",
            Owner::OtherMcp => "another idealyst mcp",
            Owner::Person => "a person",
        }
    }
}

/// Process `pid`'s parent, from `ps`.
#[cfg(unix)]
fn parent_pid(pid: u32) -> Option<u32> {
    let out = std::process::Command::new("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(not(unix))]
fn parent_pid(_pid: u32) -> Option<u32> {
    None
}

/// `GET /status`: the session and its current state.
pub fn status(entry: &SessionEntry) -> Result<serde_json::Value, String> {
    call(entry, "GET", "/status")
}

/// `POST /rebuild`: what the panel's `r` does.
pub fn rebuild(entry: &SessionEntry) -> Result<serde_json::Value, String> {
    call(entry, "POST", "/rebuild")
}

fn call(entry: &SessionEntry, method: &str, path: &str) -> Result<serde_json::Value, String> {
    let (code, body) = sessions::request(entry, method, path, REQUEST_TIMEOUT).map_err(|e| {
        format!(
            "the session (pid {}) did not answer on {}: {e}. If it is hung, `stop_dev` with \
             `pid` and `include_external: true` stops it by signal.",
            entry.pid, entry.control_url
        )
    })?;
    let value: serde_json::Value =
        serde_json::from_str(&body).unwrap_or_else(|_| serde_json::Value::String(body.clone()));
    match code {
        200..=299 => Ok(value),
        409 => Err(format!(
            "the session (pid {}) has no watcher that can rebuild — it was started with --no-build, \
             or its first build has not finished",
            entry.pid
        )),
        _ => Err(format!("the session (pid {}) answered {code}: {body}", entry.pid)),
    }
}

/// How a stop by signal ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Stopped {
    /// It exited on SIGINT — its own teardown ran (children stopped, the
    /// panel's terminal restored).
    Gracefully,
    /// It ignored SIGINT for the grace period and was killed.
    Killed,
    /// It was already gone.
    AlreadyGone,
}

/// Stop process `pid` by signal: SIGINT to its process group when it
/// leads one (an `idealyst dev` started from a shell does, so its cargo
/// and app children go with it), else to it alone; SIGKILL after `grace`.
/// Its registry entry, if left behind, is removed.
#[cfg(unix)]
pub fn stop_by_signal(pid: u32, grace: Duration) -> Stopped {
    use crate::app_discovery::pid_is_live;
    if !pid_is_live(pid) {
        remove_entry(pid);
        return Stopped::AlreadyGone;
    }
    // SAFETY: plain signal syscalls on a pid we were given.
    let pgid = unsafe { libc::getpgid(pid as i32) };
    let target = if pgid == pid as i32 { -(pid as i32) } else { pid as i32 };
    unsafe {
        libc::kill(target, libc::SIGINT);
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if !pid_is_live(pid) {
            remove_entry(pid);
            return Stopped::Gracefully;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    unsafe {
        libc::kill(target, libc::SIGKILL);
    }
    let reaped = Instant::now() + Duration::from_secs(2);
    while pid_is_live(pid) && Instant::now() < reaped {
        std::thread::sleep(Duration::from_millis(50));
    }
    remove_entry(pid);
    Stopped::Killed
}

#[cfg(not(unix))]
pub fn stop_by_signal(_pid: u32, _grace: Duration) -> Stopped {
    Stopped::AlreadyGone
}

fn remove_entry(pid: u32) {
    if let Some(dir) = sessions::registry_dir() {
        let _ = std::fs::remove_file(sessions::entry_path(&dir, pid));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A crashed session's entry is not reported, and is cleaned away.
    #[test]
    fn a_dead_sessions_entry_is_pruned() {
        let dir = std::env::temp_dir().join(format!("idealyst-live-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let me = SessionEntry::new(std::process::id(), "/w/a", "http://127.0.0.1:9", "t");
        sessions::write(&dir, &me).unwrap();
        // A pid that cannot be running: past the default pid_max.
        let dead = SessionEntry::new(4_000_000, "/w/b", "http://127.0.0.1:9", "t");
        let dead_path = sessions::write(&dir, &dead).unwrap();
        let live = live_in(&dir);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].entry.pid, std::process::id());
        assert!(!dead_path.exists(), "the dead session's entry was left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression: after an `idealyst mcp` update, the new server called
    /// the session the old one had started "another idealyst mcp" and
    /// refused to stop it without `include_external`. A session whose
    /// launching server has exited is this server's to stop; one whose
    /// launcher still runs is not.
    #[test]
    fn regression_a_session_whose_mcp_exited_is_this_servers() {
        let mut e = SessionEntry::new(4242, "/w/app", "http://127.0.0.1:9", "t");
        let no_parent = |_| None;
        assert_eq!(Owner::with_parent(&e, None, no_parent), Owner::Person);
        assert_eq!(Owner::with_parent(&e, Some(3), no_parent), Owner::ThisServer(3));

        e.launched_by = "mcp".into();
        // Its launcher recorded, and gone (past the default pid_max).
        e.launcher_pid = Some(4_000_000);
        assert_eq!(Owner::with_parent(&e, None, no_parent), Owner::ExitedMcp);
        assert!(!Owner::ExitedMcp.is_external());
        // Its launcher recorded, and running: another agent's.
        e.launcher_pid = Some(std::process::id());
        assert_eq!(Owner::with_parent(&e, None, no_parent), Owner::OtherMcp);
        assert!(Owner::OtherMcp.is_external());

        // An entry from before the field: re-parented to init means the
        // launcher is gone; a live parent or an unknown one means not.
        e.launcher_pid = None;
        assert_eq!(Owner::with_parent(&e, None, |_| Some(1)), Owner::ExitedMcp);
        assert_eq!(Owner::with_parent(&e, None, |_| Some(std::process::id())), Owner::OtherMcp);
        assert_eq!(Owner::with_parent(&e, None, no_parent), Owner::OtherMcp);
    }

    /// The parent lookup the fallback above relies on.
    #[test]
    fn a_child_names_this_process_as_its_parent() {
        let mut child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        assert_eq!(parent_pid(child.id()), Some(std::process::id()));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    /// A process that exits on SIGINT stops gracefully; one that ignores
    /// it is killed after the grace period.
    #[test]
    fn a_session_is_stopped_by_signal_and_killed_if_it_ignores_it() {
        // Reaped as soon as they exit, as a session's own parent (its
        // shell) would: an unreaped child stays a zombie, which still
        // answers `kill(pid, 0)`.
        let reaped = |mut child: std::process::Child| {
            let pid = child.id();
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            pid
        };
        let pid = reaped(std::process::Command::new("sleep").arg("30").spawn().unwrap());
        assert_eq!(stop_by_signal(pid, Duration::from_secs(5)), Stopped::Gracefully);

        // `trap '' INT` ignores SIGINT; only SIGKILL ends it.
        let pid = reaped(
            std::process::Command::new("sh")
                .args(["-c", "trap '' INT; while :; do sleep 0.1; done"])
                .spawn()
                .unwrap(),
        );
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(stop_by_signal(pid, Duration::from_millis(500)), Stopped::Killed);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(stop_by_signal(pid, Duration::from_millis(100)), Stopped::AlreadyGone);
    }
}
