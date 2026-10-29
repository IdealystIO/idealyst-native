//! The session's control endpoint and its entry in the machine's session
//! registry (`dev_events::sessions`), so a tool — `idealyst mcp`, for an
//! agent — finds this session and drives it as it would one it started:
//! reads its state, requests a rebuild (the panel's `r`), and finds its
//! log. Stopping goes by the pid in the entry, so it works on a session
//! too wedged to answer.
//!
//! It also keeps a project to one session ([`claim_dir`]): a second
//! session of a directory would share the first's target directory, log
//! and metrics file, and its cargo builds would race the first's.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dev_events::sessions::{self, SessionEntry};

/// What the endpoint does for the session: read its state, and request a
/// rebuild (returning the request's number and the targets that took
/// it). Supplied by the caller so the request handling is testable
/// without a session.
pub struct Actions {
    pub status: Box<dyn Fn() -> Vec<serde_json::Value> + Send + Sync>,
    pub rebuild: Box<dyn Fn() -> crate::dev_log::Requested + Send + Sync>,
}

/// Answer one request: `(status code, JSON body)`. `authorization` is the
/// request's header, `has_origin` whether it carried an `Origin` header
/// (a browser page's request always does; a local tool's never).
pub fn handle(
    method: &str,
    path: &str,
    authorization: Option<&str>,
    has_origin: bool,
    entry: &SessionEntry,
    actions: &Actions,
) -> (u16, String) {
    if has_origin {
        return (403, r#"{"error":"a request from a web page is refused"}"#.into());
    }
    let expected = format!("Bearer {}", entry.token);
    if authorization != Some(expected.as_str()) {
        return (401, r#"{"error":"missing or wrong token; read it from this session's registry entry"}"#.into());
    }
    match (method, path) {
        ("GET", "/status") => {
            let body = serde_json::json!({ "session": entry.public(), "events": (actions.status)() });
            (200, body.to_string())
        }
        ("POST", "/rebuild") => {
            // `request` is what a wait on this rebuild's outcome names
            // (`RebuildTaken` events carry it).
            let r = (actions.rebuild)();
            let body = serde_json::json!({
                "accepted": r.targets.len(),
                "request": r.request,
                "targets": r.targets,
                "no_watch": entry.no_watch,
            });
            (if r.targets.is_empty() { 409 } else { 202 }, body.to_string())
        }
        _ => (404, r#"{"error":"GET /status or POST /rebuild"}"#.into()),
    }
}

/// Removes the session's registry entry when the session ends normally
/// (the Ctrl-C path, which exits the process, removes it from the
/// session's exit-file list instead).
pub struct Registration {
    path: PathBuf,
}

impl Registration {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Start the control endpoint on an ephemeral loopback port and register
/// the session (`entry` filled in but for its address and token, which
/// this supplies). `None` when there is no registry directory or the
/// endpoint cannot start: the session runs anyway, just undiscoverable.
pub fn start(entry: SessionEntry, actions: Actions) -> Option<Registration> {
    start_in(&sessions::registry_dir()?, entry, actions)
}

/// [`start`], registering into `registry`.
pub fn start_in(registry: &Path, mut entry: SessionEntry, actions: Actions) -> Option<Registration> {
    let server = tiny_http::Server::http("127.0.0.1:0").ok()?;
    let port = server.server_addr().to_ip()?.port();
    entry.control_url = format!("http://127.0.0.1:{port}");
    entry.token = token();
    let path = sessions::write(registry, &entry).ok()?;
    let entry = Arc::new(entry);
    std::thread::Builder::new()
        .name("dev-control".into())
        .spawn(move || {
            for request in server.incoming_requests() {
                let header = |name: &str| {
                    request
                        .headers()
                        .iter()
                        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
                        .map(|h| h.value.as_str().to_string())
                };
                let authorization = header("authorization");
                let has_origin = header("origin").is_some();
                let method = request.method().as_str().to_string();
                let path = request.url().split('?').next().unwrap_or("").to_string();
                let (code, body) = handle(&method, &path, authorization.as_deref(), has_origin, &entry, &actions);
                let json = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .expect("a valid header");
                let _ = request.respond(tiny_http::Response::from_string(body).with_status_code(code).with_header(json));
            }
        })
        .ok()?;
    Some(Registration { path })
}

/// A project directory claimed for this session; dropping it (or the
/// process ending, however it ends) releases the claim.
pub struct DirClaim {
    _lock: std::fs::File,
}

/// Claim `dir` for this session, or say which session already has it.
///
/// Two checks, because neither covers every case alone. The registry
/// (`registry`) names a running session of `dir`, but a session's entry
/// is written seconds after it starts, so two sessions started together
/// would both find it empty. The lock on `lock_path` (beside the session
/// log, `dev.lock`) closes that window, and the OS drops it with the
/// process, so a crash cannot leave the directory claimed. It is taken
/// only by an `idealyst` that knows it, so an older session is found
/// through the registry alone.
pub fn claim_dir(dir: &Path, lock_path: &Path, registry: Option<&Path>) -> anyhow::Result<DirClaim> {
    if let Some(registry) = registry {
        let others: Vec<SessionEntry> = sessions::read_all(registry)
            .into_iter()
            .map(|(_, e)| e)
            .filter(|e| e.pid != std::process::id() && pid_is_live(e.pid))
            .collect();
        if let Some(e) = sessions::in_dir(&others, dir) {
            let who = if e.launched_by == "mcp" { "an agent, through `idealyst mcp`" } else { "a person" };
            anyhow::bail!(
                "an `idealyst dev` session already runs in {} (pid {}, started by {who}, idealyst {}). \
                 Two sessions of one project share its target directory and log, and their builds \
                 race. Use that session — the idealyst MCP drives it by pid (`dev_status`, \
                 `rebuild_dev`, `wait_for_build`) — or stop it first (Ctrl-C in its terminal, \
                 `kill -INT {}`, or `stop_dev` with its pid).",
                dir.display(),
                e.pid,
                e.cli_version,
                e.pid,
            );
        }
    }
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)
        .map_err(|e| anyhow::anyhow!("cannot open {}: {e}", lock_path.display()))?;
    match lock.try_lock() {
        Ok(()) => Ok(DirClaim { _lock: lock }),
        Err(std::fs::TryLockError::WouldBlock) => anyhow::bail!(
            "another `idealyst dev` session is starting in {} (it holds {}). Two sessions of one \
             project share its target directory and log, and their builds race: wait for it and \
             use it (`list_dev_sessions` shows it once it is up), or stop it first.",
            dir.display(),
            lock_path.display(),
        ),
        Err(std::fs::TryLockError::Error(e)) => {
            anyhow::bail!("cannot lock {}: {e}", lock_path.display())
        }
    }
}

/// Whether process `pid` exists. Off unix every entry counts as live,
/// which only matters for an entry a crash left behind.
#[cfg(unix)]
fn pid_is_live(pid: u32) -> bool {
    // SAFETY: signal 0 checks for existence and delivers nothing.
    let rc = unsafe { libc::kill(pid as i32, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn pid_is_live(_pid: u32) -> bool {
    true
}

/// 32 hex characters from the OS's randomness. A process that can read
/// `/dev/urandom` but not this user's entry file cannot guess it.
fn token() -> String {
    let mut bytes = [0u8; 16];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes));
    if read.is_err() {
        // No /dev/urandom (Windows): the std hasher's per-process random
        // keys, over the time and the pid.
        use std::hash::{BuildHasher, Hasher};
        for chunk in bytes.chunks_mut(8) {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
            h.write_u32(std::process::id());
            chunk.copy_from_slice(&h.finish().to_le_bytes()[..chunk.len()]);
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn actions(rebuilds: Arc<AtomicUsize>) -> Actions {
        Actions {
            status: Box::new(|| vec![serde_json::json!({"type": "session_started"})]),
            rebuild: Box::new(move || {
                let n = rebuilds.fetch_add(1, Ordering::SeqCst) as u64 + 1;
                crate::dev_log::Requested { request: n, targets: vec!["web".into()] }
            }),
        }
    }

    fn entry() -> SessionEntry {
        let mut e = SessionEntry::new(7, "/w/app", "http://127.0.0.1:0", "tok");
        e.app = "app".into();
        e.no_watch = true;
        e
    }

    #[test]
    fn the_token_is_required_and_a_web_page_is_refused() {
        let n = Arc::new(AtomicUsize::new(0));
        let a = actions(n.clone());
        let e = entry();
        assert_eq!(handle("POST", "/rebuild", None, false, &e, &a).0, 401);
        assert_eq!(handle("POST", "/rebuild", Some("Bearer nope"), false, &e, &a).0, 401);
        assert_eq!(handle("POST", "/rebuild", Some("Bearer tok"), true, &e, &a).0, 403, "a page carrying the token");
        assert_eq!(n.load(Ordering::SeqCst), 0, "a refused request rebuilt");
        let (code, body) = handle("POST", "/rebuild", Some("Bearer tok"), false, &e, &a);
        assert_eq!(code, 202);
        assert_eq!(n.load(Ordering::SeqCst), 1);
        assert!(body.contains(r#""accepted":1"#), "{body}");
        assert!(body.contains(r#""request":1"#), "the request's number is what a wait names: {body}");
        let (code, body) = handle("GET", "/status", Some("Bearer tok"), false, &e, &a);
        assert_eq!(code, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["session"]["app"], "app");
        assert!(v["session"].get("token").is_none(), "the status leaks the token");
        assert_eq!(v["events"][0]["type"], "session_started");
        assert_eq!(handle("DELETE", "/status", Some("Bearer tok"), false, &e, &a).0, 404);
    }

    /// Regression: a session started while another still ran in the same
    /// directory, sharing its target directory and log. The second is
    /// refused, whether the first is found by its registry entry (an
    /// older `idealyst`, which takes no lock) or by its lock (one still
    /// starting, not yet registered).
    #[cfg(unix)]
    #[test]
    fn regression_a_second_session_of_one_directory_is_refused() {
        let root = std::env::temp_dir().join(format!("idealyst-claim-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (app, registry) = (root.join("app"), root.join("registry"));
        std::fs::create_dir_all(&app).unwrap();
        let lock = app.join("target/idealyst/app/dev.lock");

        // A running session's entry: a live process standing in for it.
        let mut running = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let mut e = SessionEntry::new(running.id(), app.display().to_string(), "http://127.0.0.1:9", "t");
        e.launched_by = "mcp".into();
        e.cli_version = "1.19.0".into();
        sessions::write(&registry, &e).unwrap();
        let err = claim_dir(&app, &lock, Some(&registry)).err().expect("refused beside a running session");
        let text = err.to_string();
        assert!(text.contains(&format!("pid {}", running.id())), "{text}");
        assert!(text.contains("an agent"), "{text}");
        running.kill().unwrap();
        running.wait().unwrap();

        // Its process gone, the entry does not hold the directory.
        let first = claim_dir(&app, &lock, Some(&registry)).expect("claimed once the session exited");
        // One still starting holds the lock before it registers.
        let err = claim_dir(&app, &lock, Some(&registry)).err().expect("refused beside a starting session");
        assert!(err.to_string().contains("is starting"), "{err}");
        drop(first);
        // A process this test binary forks concurrently (other tests spawn
        // children) shares the lock's open file description until its exec
        // closes it (`O_CLOEXEC`), so the release can lag by that instant.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let reclaimed = loop {
            match claim_dir(&app, &lock, Some(&registry)) {
                Ok(c) => break c,
                Err(e) if std::time::Instant::now() > deadline => panic!("released with the claim: {e}"),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        };
        drop(reclaimed);
        // Another directory is another project.
        let other = root.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let _a = loop {
            match claim_dir(&app, &lock, Some(&registry)) {
                Ok(c) => break c,
                Err(e) if std::time::Instant::now() > deadline => panic!("{e}"),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        };
        claim_dir(&other, &other.join("dev.lock"), Some(&registry)).expect("a different project");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The whole path a tool takes: find the entry, request through it.
    #[test]
    fn a_registered_session_is_driven_through_its_entry() {
        let registry = std::env::temp_dir().join(format!("idealyst-control-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&registry);
        let n = Arc::new(AtomicUsize::new(0));
        let reg = start_in(&registry, entry(), actions(n.clone())).expect("the endpoint starts");
        let found = sessions::read_all(&registry);
        assert_eq!(found.len(), 1);
        let (path, e) = &found[0];
        assert_eq!(path, reg.path());
        assert_eq!(e.token.len(), 32);
        let (code, _) = sessions::request(e, "POST", "/rebuild", Duration::from_secs(5)).unwrap();
        assert_eq!(code, 202);
        assert_eq!(n.load(Ordering::SeqCst), 1);
        let (code, body) = sessions::request(e, "GET", "/status", Duration::from_secs(5)).unwrap();
        assert_eq!(code, 200, "{body}");
        drop(reg);
        assert!(sessions::read_all(&registry).is_empty(), "the entry outlived the session");
        let _ = std::fs::remove_dir_all(&registry);
    }
}
