//! A thin TCP client for the Robot bridge / relay — speaks the same
//! newline-delimited JSON the MCP server and the arena evaluator use, so the
//! test runner reaches a web, macOS, iOS, or Android app identically (the relay
//! exposes them all as one TCP bridge).

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long [`RobotClient::call`] waits for a reply by default.
pub const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(8);

/// How much sooner than the client the relay is asked to give up (the
/// envelope's `timeout_ms`), so a busy app is reported by the relay — which
/// knows the app is the slow side — before the client's own read times out.
const RELAY_MARGIN: Duration = Duration::from_millis(500);

/// Why a [`RobotClient::call`] failed. Carried inside the `anyhow::Error`
/// it returns; [`CallError::of`] finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// The relay is up and the app connected, but the app did not answer in
    /// time — busy on its UI thread, or hung. The relay's own words.
    AppTimedOut(String),
    /// No reply reached this client within its I/O timeout: the relay (or a
    /// direct bridge) is unreachable or stuck — or, on a direct bridge, the
    /// app itself is busy, since nothing sits between them to say which.
    ClientTimedOut { verb: String, waited: Duration },
    /// The relay has no app to forward to (it has not dialed in yet, or is
    /// reloading).
    NoApp(String),
    /// The app answered with an error.
    Verb(String),
}

impl CallError {
    /// The [`CallError`] inside a `call`'s error, if it is one (I/O and
    /// protocol failures are not).
    pub fn of(err: &anyhow::Error) -> Option<&CallError> {
        err.downcast_ref::<CallError>()
    }

    /// Whether the same request could succeed if sent again shortly: the
    /// app was busy or not yet connected — "not yet", for a caller that is
    /// waiting anyway.
    pub fn is_transient(&self) -> bool {
        !matches!(self, CallError::Verb(_))
    }

    fn classify(verb: &str, message: String) -> Self {
        if message.starts_with("app response timed out") {
            CallError::AppTimedOut(message)
        } else if message.starts_with("no app connected") || message == "app disconnected" {
            CallError::NoApp(message)
        } else {
            let _ = verb;
            CallError::Verb(message)
        }
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::AppTimedOut(m) => write!(f, "{m}"),
            CallError::ClientTimedOut { verb, waited } => write!(
                f,
                "no reply to `{verb}` within {waited:?}: the client gave up waiting on the relay/bridge \
                 (raise it with App::set_io_timeout)"
            ),
            CallError::NoApp(m) => write!(f, "{m}"),
            CallError::Verb(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for CallError {}

/// Whether `err` (from [`RobotClient::call`]) means "busy or not connected
/// yet, ask again" — see [`CallError::is_transient`].
pub fn is_transient(err: &anyhow::Error) -> bool {
    CallError::of(err).is_some_and(CallError::is_transient)
}

/// One connection to a running app's Robot bridge (direct or via the relay).
pub struct RobotClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
    io_timeout: Duration,
}

impl RobotClient {
    pub fn connect(addr: SocketAddr) -> std::io::Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
        let mut client = Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            next_id: 1,
            io_timeout: DEFAULT_IO_TIMEOUT,
        };
        client.set_io_timeout(DEFAULT_IO_TIMEOUT)?;
        Ok(client)
    }

    /// How long one [`call`](Self::call) waits for its reply (default
    /// [`DEFAULT_IO_TIMEOUT`]). The relay is asked to wait a little less
    /// for the app, so a busy app comes back as [`CallError::AppTimedOut`]
    /// rather than this client timing out.
    pub fn set_io_timeout(&mut self, timeout: Duration) -> std::io::Result<()> {
        let timeout = timeout.max(Duration::from_millis(1));
        self.writer.set_read_timeout(Some(timeout))?;
        self.writer.set_write_timeout(Some(timeout))?;
        self.io_timeout = timeout;
        Ok(())
    }

    /// The per-call I/O timeout ([`set_io_timeout`](Self::set_io_timeout)).
    pub fn io_timeout(&self) -> Duration {
        self.io_timeout
    }

    /// Issue a verb; return the `ok` payload, or `Err` on a bridge `err`.
    /// A failure the bridge or relay reports, or a reply that never came,
    /// is a [`CallError`] inside the error.
    pub fn call(&mut self, cmd: &str, args: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let relay_wait = self.io_timeout.saturating_sub(RELAY_MARGIN).max(Duration::from_millis(1));
        let mut line = json!({
            "id": id,
            "cmd": cmd,
            "args": args,
            // Read by the relay; a direct bridge ignores envelope fields it
            // doesn't know.
            "timeout_ms": relay_wait.as_millis() as u64,
        })
        .to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;

        loop {
            let mut resp = String::new();
            let n = match self.reader.read_line(&mut resp) {
                Ok(n) => n,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    return Err(CallError::ClientTimedOut { verb: cmd.to_string(), waited: self.io_timeout }.into());
                }
                Err(e) => return Err(e.into()),
            };
            if n == 0 {
                anyhow::bail!("bridge closed the connection");
            }
            let v: Value = serde_json::from_str(resp.trim())?;
            // A reply to an earlier call this client stopped waiting for
            // (it timed out), or a `changed` push: not ours, skip it.
            // Without this, every call after one timeout read the previous
            // call's answer.
            if v.get("id").and_then(|i| i.as_u64()) != Some(id) {
                continue;
            }
            if let Some(err) = v.get("err") {
                let message = err.as_str().map(str::to_string).unwrap_or_else(|| err.to_string());
                return Err(CallError::classify(cmd, message).into());
            }
            return Ok(v.get("ok").cloned().unwrap_or(Value::Null));
        }
    }

    /// Wait until the app answers a `ping` (it has dialed the relay / the bridge
    /// is up), or the timeout elapses.
    pub fn wait_ready(&mut self, timeout: Duration) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(v) = self.call("ping", json!({})) {
                if v == json!("pong") {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                anyhow::bail!("app not ready within {timeout:?}");
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Default app-registry directory: `~/.idealyst/apps`.
pub fn default_apps_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".idealyst").join("apps"))
}

/// Name of the registry directory, beside [`default_apps_dir`], that Windows
/// apps running under Wine register into. Wine PIDs are not host PIDs, and
/// the bridge prunes registrations by PID liveness, so the two kinds of
/// process cannot share a directory (see `IDEALYST_APPS_DIR` in
/// `runtime_shared::robot::bridge`). `run-windows` sets the same name.
pub const WINE_APPS_DIR_NAME: &str = "apps-wine";

/// Every registry directory a host-side tool should scan: the native one and
/// the Wine one. Discovery here checks liveness by TCP connect, never by PID,
/// so reading the Wine directory from the host is safe.
pub fn default_apps_dirs() -> Vec<PathBuf> {
    let Some(native) = default_apps_dir() else { return Vec::new() };
    let wine = native.with_file_name(WINE_APPS_DIR_NAME);
    vec![native, wine]
}

#[derive(serde::Deserialize)]
struct AppReg {
    port: u16,
    #[serde(default)]
    project_root: String,
    /// The app belongs to an `idealyst test` run (its relay writes this):
    /// that run's suite drives it alone.
    #[serde(default)]
    under_test: bool,
}

/// Discover a running app's bridge address. If `project_dir` is given, prefer
/// the app whose `project_root` matches; otherwise return the first live app.
///
/// An app an `idealyst test` run launched (`"under_test"` in its
/// registration) is never returned: the run hands its suite the address
/// itself (`IDEALYST_ROBOT_BRIDGE`), and anything that found the app here
/// instead — a bare `cargo test` of the same suite — would drive it beside
/// the run, each resetting state the other is asserting on.
pub fn discover(project_dir: Option<&Path>, apps_dir: &Path) -> Option<SocketAddr> {
    let want = project_dir
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    let mut fallback = None;
    for entry in std::fs::read_dir(apps_dir).ok()?.flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(reg) = std::fs::read_to_string(entry.path())
            .ok()
            .and_then(|s| serde_json::from_str::<AppReg>(&s).ok())
            .ok_or(())
        else {
            continue;
        };
        if reg.under_test {
            continue;
        }
        let Ok(addr) = format!("127.0.0.1:{}", reg.port).parse::<SocketAddr>() else {
            continue;
        };
        if TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_err() {
            continue; // stale registration
        }
        if let Some(want) = &want {
            let reg_root = std::fs::canonicalize(&reg.project_root)
                .unwrap_or_else(|_| PathBuf::from(&reg.project_root));
            if &reg_root == want {
                return Some(addr);
            }
        }
        fallback.get_or_insert(addr);
    }
    fallback
}

/// The **live** app bridge registered for `project_dir` — and only that one.
/// Unlike [`discover`], no fallback to another project's app: a launcher
/// waiting for ITS app to register must not settle for whatever else is
/// running. (`idealyst test` did, and with another `idealyst dev` session up
/// it pinged that session's relay for four minutes and reported "the app did
/// not become ready".)
pub fn discover_project(project_dir: &Path, apps_dir: &Path) -> Option<SocketAddr> {
    discover_all(Some(project_dir), apps_dir).into_iter().next()
}

/// Every **live** app bridge registered for `project_dir` (or every live app
/// when `project_dir` is `None`). Unlike [`discover`], which returns one, this
/// returns all of them — so a caller running two platforms of the same project
/// can tell the two apps apart by which registration appeared after each
/// launch (the registration file carries no platform field). Stale entries
/// (their port no longer accepts a connection) are skipped.
pub fn discover_all(project_dir: Option<&Path>, apps_dir: &Path) -> Vec<SocketAddr> {
    let want = project_dir
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(apps_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(reg) = std::fs::read_to_string(entry.path())
            .ok()
            .and_then(|s| serde_json::from_str::<AppReg>(&s).ok())
        else {
            continue;
        };
        let Ok(addr) = format!("127.0.0.1:{}", reg.port).parse::<SocketAddr>() else {
            continue;
        };
        if let Some(want) = &want {
            let reg_root = std::fs::canonicalize(&reg.project_root)
                .unwrap_or_else(|_| PathBuf::from(&reg.project_root));
            if &reg_root != want {
                continue;
            }
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_err() {
            continue; // stale registration
        }
        out.push(addr);
    }
    out
}
