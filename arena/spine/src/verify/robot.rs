//! Robot tier: assert against the running app's **self-report** — the same
//! Robot bridge the implementation agent introspects through, queried here
//! independently by the evaluator.
//!
//! The bridge is native-only (it can't run in a wasm/web build), so Robot-tier
//! items target a native run of the app (macOS on this host). A running app
//! registers itself in `~/.idealyst/apps/<name>-<pid>.json` and listens on a
//! TCP port speaking newline-delimited JSON:
//!
//! ```text
//! → {"id":1,"cmd":"find_element","args":{"label_contains":"Buy milk"}}\n
//! ← {"id":1,"ok":{...}}            // or {"id":1,"err":"..."}
//! ```
//!
//! This is a thin, dependency-free client over `std::net` — no framework path
//! deps required.

use super::{RunContext, VerifyResult, Verifier};
use crate::rubric::RubricItem;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// A live connection to one app's Robot bridge.
pub struct RobotClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

impl RobotClient {
    pub fn connect(addr: SocketAddr) -> std::io::Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let writer = stream.try_clone()?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
            next_id: 1,
        })
    }

    /// Issue one verb. Returns the `ok` payload, or `Err` if the bridge replied
    /// with `err` (or the framing was malformed).
    pub fn call(&mut self, cmd: &str, args: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({ "id": id, "cmd": cmd, "args": args });
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;

        let mut resp_line = String::new();
        let n = self.reader.read_line(&mut resp_line)?;
        if n == 0 {
            anyhow::bail!("robot bridge closed the connection");
        }
        let resp: Value = serde_json::from_str(resp_line.trim())?;
        if let Some(err) = resp.get("err") {
            anyhow::bail!("robot `{cmd}` error: {err}");
        }
        Ok(resp.get("ok").cloned().unwrap_or(Value::Null))
    }
}

/// Default app-registry directory: `~/.idealyst/apps`.
pub fn default_apps_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".idealyst").join("apps"))
}

/// One app registration file's contents.
#[derive(Debug, serde::Deserialize)]
struct AppRegistration {
    port: u16,
    #[serde(default)]
    project_root: String,
}

/// Find the bridge address of a running native app whose `project_root` matches
/// `project_dir`, preferring one we can actually connect to. Returns `None`
/// when no live, matching app is registered.
pub fn discover(project_dir: &Path, apps_dir: &Path) -> Option<SocketAddr> {
    let want = std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
    let entries = std::fs::read_dir(apps_dir).ok()?;
    let mut fallback: Option<SocketAddr> = None;

    for entry in entries.flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(reg) = serde_json::from_str::<AppRegistration>(&raw) else {
            continue;
        };
        let addr: SocketAddr = match format!("127.0.0.1:{}", reg.port).parse() {
            Ok(a) => a,
            Err(_) => continue,
        };
        // Liveness: a registration whose port no longer accepts connections is
        // stale (the app died without cleaning up).
        if TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).is_err() {
            continue;
        }
        let reg_root = std::fs::canonicalize(&reg.project_root)
            .unwrap_or_else(|_| PathBuf::from(&reg.project_root));
        if reg_root == want {
            return Some(addr);
        }
        fallback.get_or_insert(addr);
    }
    // No project_root match — fall back to any live app (single-app hosts).
    fallback
}

pub struct RobotVerifier;

impl Verifier for RobotVerifier {
    fn verify(&self, item: &RubricItem, ctx: &RunContext) -> VerifyResult {
        let Some(apps_dir) = default_apps_dir() else {
            return VerifyResult::skip("no HOME — cannot locate the robot app registry");
        };
        let Some(addr) = discover(&ctx.project_dir, &apps_dir) else {
            return VerifyResult::skip(
                "no running native app found on the robot bridge (run the app on a native target first)",
            );
        };
        let mut client = match RobotClient::connect(addr) {
            Ok(c) => c,
            Err(e) => return VerifyResult::skip(format!("robot bridge connect failed: {e}")),
        };
        interpret(item, &mut client)
    }
}

/// Map a rubric item's assertion onto bridge verbs. Two shapes are supported:
///   * explicit `verb` (+ optional `expect_name` substring check on the result)
///   * `expect_name` alone → `find_element{label_contains}` must locate something
fn interpret(item: &RubricItem, client: &mut RobotClient) -> VerifyResult {
    let a = &item.assertion;
    if let Some(verb) = &a.verb {
        match client.call(verb, json!({})) {
            Ok(result) => {
                if let Some(name) = &a.expect_name {
                    if result.to_string().contains(name) {
                        VerifyResult::pass(format!("`{verb}` result contains `{name}`"))
                    } else {
                        VerifyResult::fail(format!("`{verb}` result missing `{name}`: {result}"))
                    }
                } else {
                    VerifyResult::pass(format!("`{verb}` ok: {result}"))
                }
            }
            Err(e) => VerifyResult::fail(format!("{e}")),
        }
    } else if let Some(name) = &a.expect_name {
        match client.call("find_element", json!({ "label_contains": name })) {
            Ok(Value::Null) => VerifyResult::fail(format!("no element labelled `{name}` found")),
            Ok(found) => VerifyResult::pass(format!("found element labelled `{name}`: {found}")),
            Err(e) => VerifyResult::fail(format!("find_element failed: {e}")),
        }
    } else {
        VerifyResult::fail("robot item needs `verb` or `expect_name` in its assertion")
    }
}
