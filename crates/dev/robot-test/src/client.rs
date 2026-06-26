//! A thin TCP client for the Robot bridge / relay — speaks the same
//! newline-delimited JSON the MCP server and the arena evaluator use, so the
//! test runner reaches a web, macOS, iOS, or Android app identically (the relay
//! exposes them all as one TCP bridge).

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const IO_TIMEOUT: Duration = Duration::from_secs(8);

/// One connection to a running app's Robot bridge (direct or via the relay).
pub struct RobotClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

impl RobotClient {
    pub fn connect(addr: SocketAddr) -> std::io::Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        Ok(Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            next_id: 1,
        })
    }

    /// Issue a verb; return the `ok` payload, or `Err` on a bridge `err`.
    pub fn call(&mut self, cmd: &str, args: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = json!({ "id": id, "cmd": cmd, "args": args }).to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;

        let mut resp = String::new();
        let n = self.reader.read_line(&mut resp)?;
        if n == 0 {
            anyhow::bail!("bridge closed the connection");
        }
        let v: Value = serde_json::from_str(resp.trim())?;
        if let Some(err) = v.get("err") {
            anyhow::bail!("{}", err.as_str().unwrap_or(&err.to_string()));
        }
        Ok(v.get("ok").cloned().unwrap_or(Value::Null))
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

#[derive(serde::Deserialize)]
struct AppReg {
    port: u16,
    #[serde(default)]
    project_root: String,
}

/// Discover a running app's bridge address. If `project_dir` is given, prefer
/// the app whose `project_root` matches; otherwise return the first live app.
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
