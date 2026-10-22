//! The running `idealyst dev` sessions of this machine, and the way into
//! each one.
//!
//! Every session — one a person started in a terminal, or one `idealyst
//! mcp`'s `run_dev` started for an agent — writes a [`SessionEntry`] to
//! `~/.idealyst/dev-sessions/<pid>.json` ([`registry_dir`]) and removes it
//! when it exits. The entry names the session's project, its log, its
//! event stream, and its CONTROL endpoint: a loopback HTTP server the
//! session runs that answers
//!
//! - `GET /status` — the session as JSON: this entry (without the token)
//!   and its current state as the events that describe it (the same
//!   snapshot `/__idealyst/events` opens with);
//! - `POST /rebuild` — the panel's `r`: every watcher of the session
//!   rebuilds, or under `--no-watch` applies the saves it queued.
//!
//! Every request carries `Authorization: Bearer <token>` ([`request`]
//! sends it). A web page can reach any loopback port, so the endpoint
//! refuses a request without the token, and one that carries an `Origin`
//! header at all: only a local process that can read this user's entry
//! file (written owner-only) can drive the session.
//!
//! A session that crashed leaves its entry behind; a reader checks the
//! pid before trusting one (`idealyst mcp` prunes dead ones).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The entry format's version. A reader skips an entry of another one.
pub const ENTRY_VERSION: u32 = 1;

/// One running session. See the module docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SessionEntry {
    pub version: u32,
    /// The `idealyst dev` process.
    pub pid: u32,
    /// The project directory.
    pub dir: String,
    /// The app's name (its manifest's `[app] name`).
    pub app: String,
    /// The platforms it runs (`web`, `ios`, …).
    pub targets: Vec<String>,
    /// `--local`: each platform builds the app itself, rather than
    /// streaming from a runtime-server host.
    pub local: bool,
    /// `--no-watch`: saves wait for a rebuild request.
    pub no_watch: bool,
    /// `--experimental-build`: additive hot patches.
    pub experimental_build: bool,
    /// Unix seconds.
    pub started_at: u64,
    /// The `idealyst` that runs it.
    pub cli_version: String,
    /// The session log (`dev.log`).
    pub log_file: Option<String>,
    /// Its event stream (`/__idealyst/events`, server-sent events).
    pub events_url: Option<String>,
    /// Its control endpoint, `http://127.0.0.1:<port>`.
    pub control_url: String,
    /// The bearer token every control request carries.
    pub token: String,
    /// Who started it: `"user"`, or `"mcp"` for `idealyst mcp`'s
    /// `run_dev` ([`LAUNCHED_BY_ENV`]).
    pub launched_by: String,
}

/// Set by whatever starts `idealyst dev` on someone's behalf, to say so
/// in the session's entry: `idealyst mcp` sets it to `mcp`.
pub const LAUNCHED_BY_ENV: &str = "IDEALYST_DEV_LAUNCHED_BY";

impl SessionEntry {
    /// An entry for `pid`'s session of `dir`, reachable at `control_url`
    /// with `token`. The rest is filled in by the caller, field by field.
    pub fn new(pid: u32, dir: impl Into<String>, control_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            version: ENTRY_VERSION,
            pid,
            dir: dir.into(),
            app: String::new(),
            targets: Vec::new(),
            local: false,
            no_watch: false,
            experimental_build: false,
            started_at: 0,
            cli_version: String::new(),
            log_file: None,
            events_url: None,
            control_url: control_url.into(),
            token: token.into(),
            launched_by: "user".into(),
        }
    }

    /// This entry as `GET /status` shows it: the token left out.
    pub fn public(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if let Some(o) = v.as_object_mut() {
            o.remove("token");
        }
        v
    }
}

/// `~/.idealyst/dev-sessions`. `None` without a home directory.
pub fn registry_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".idealyst").join("dev-sessions"))
}

/// Where `pid`'s entry lives in `dir`.
pub fn entry_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.json"))
}

/// Write `entry` into `dir`, readable by its owner only (it holds the
/// token), replacing any entry of the same pid. Returns the file.
pub fn write(dir: &Path, entry: &SessionEntry) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = entry_path(dir, entry.pid);
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(entry).map_err(std::io::Error::other)?;
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(&json)?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Every entry in `dir` of this format version, with its file. Dead
/// sessions included: the reader decides (see the module docs).
pub fn read_all(dir: &Path) -> Vec<(PathBuf, SessionEntry)> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(PathBuf, SessionEntry)> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|p| {
            let entry: SessionEntry = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            (entry.version == ENTRY_VERSION).then_some((p, entry))
        })
        .collect();
    out.sort_by_key(|(_, e)| (e.started_at, e.pid));
    out
}

/// One request to a session's control endpoint: `method` `path` with the
/// session's token. Returns the status code and the body. `timeout`
/// bounds connecting and each read and write.
pub fn request(entry: &SessionEntry, method: &str, path: &str, timeout: Duration) -> std::io::Result<(u16, String)> {
    let authority = entry
        .control_url
        .strip_prefix("http://")
        .ok_or_else(|| std::io::Error::other(format!("not an http control url: {}", entry.control_url)))?
        .trim_end_matches('/');
    let addr = std::net::ToSocketAddrs::to_socket_addrs(authority)?
        .next()
        .ok_or_else(|| std::io::Error::other(format!("no address for {authority}")))?;
    let mut stream = std::net::TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        entry.token
    )?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| std::io::Error::other("no HTTP status line in the reply"))?;
    Ok((status, body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_round_trips_and_its_status_view_hides_the_token() {
        let dir = std::env::temp_dir().join(format!("idealyst-sessions-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = SessionEntry::new(4242, "/w/app", "http://127.0.0.1:9", "secret");
        e.app = "app".into();
        e.no_watch = true;
        let path = write(&dir, &e).unwrap();
        assert_eq!(path, entry_path(&dir, 4242));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(read_all(&dir), vec![(path.clone(), e.clone())]);
        let public = e.public();
        assert!(public.get("token").is_none());
        assert_eq!(public["no_watch"], true);
        // Another version's entry is not ours to read.
        std::fs::write(dir.join("1.json"), r#"{"version":99,"pid":1}"#).unwrap();
        assert_eq!(read_all(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_request_carries_the_token_and_returns_status_and_body() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 256];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
            }
            s.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\n\r\nok").unwrap();
            s.shutdown(std::net::Shutdown::Write).unwrap();
            String::from_utf8_lossy(&req).into_owned()
        });
        let e = SessionEntry::new(1, "/w", format!("http://127.0.0.1:{port}"), "tok");
        let (status, body) = request(&e, "POST", "/rebuild", Duration::from_secs(5)).unwrap();
        assert_eq!((status, body.as_str()), (202, "ok"));
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /rebuild HTTP/1.1\r\n"), "{req}");
        assert!(req.contains("Authorization: Bearer tok\r\n"), "{req}");
    }
}
