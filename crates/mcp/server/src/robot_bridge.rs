//! Async TCP client for the running app's Robot bridge.
//!
//! Tokio-flavored so it shares the rmcp server's runtime instead of
//! standing up a second IO thread. Wire format is the bridge's
//! newline-delimited JSON `{ id, cmd, args }` ⇄ `{ id, ok/err }`
//! (see `runtime_core::robot::bridge`).
//!
//! Connection is **lazy and self-healing**: the first tool call
//! `connect()`s; subsequent calls reuse the socket; if the app
//! restarts mid-session the next failed write triggers a reconnect.
//! The MCP server stays up across app restarts — matching the
//! "doesn't go down" property phase 5 is built around.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::Mutex,
};

pub const DEFAULT_BRIDGE: &str = "127.0.0.1:9718";

/// Long-lived handle to the app's Robot bridge. All Robot tools on
/// `CatalogService` route through here. Cheap to clone (just an
/// `Arc<Mutex<...>>`).
pub struct RobotBridge {
    addr: String,
    inner: Mutex<Option<Connection>>,
    next_id: AtomicU64,
}

struct Connection {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
}

impl RobotBridge {
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            inner: Mutex::new(None),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Send a bridge command and return its `ok`/`err` payload.
    /// Retries once on connection failure (handles app restarts
    /// transparently). The returned `Value` is the bridge's `ok`
    /// payload; bridge-side errors come back as `Err(...)`.
    pub async fn call(&self, cmd: &str, args: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = json!({ "id": id, "cmd": cmd, "args": args });
        let line = serde_json::to_string(&request)?;

        for attempt in 0..2 {
            let mut guard = self.inner.lock().await;
            if guard.is_none() {
                match TcpStream::connect(&self.addr).await {
                    Ok(stream) => {
                        tracing::info!("robot bridge connected at {}", self.addr);
                        let (r, w) = stream.into_split();
                        *guard = Some(Connection {
                            reader: BufReader::new(r),
                            writer: w,
                        });
                    }
                    Err(e) => {
                        bail!(
                            "could not connect to robot bridge at {} ({}): is the app running with `--features robot`?",
                            self.addr,
                            e
                        );
                    }
                }
            }
            let conn = guard.as_mut().unwrap();

            // Write the request.
            if conn.writer.write_all(line.as_bytes()).await.is_err()
                || conn.writer.write_all(b"\n").await.is_err()
                || conn.writer.flush().await.is_err()
            {
                *guard = None;
                if attempt == 0 {
                    continue;
                }
                bail!("bridge write failed after reconnect");
            }

            // Read the response.
            let mut response_line = String::new();
            if conn.reader.read_line(&mut response_line).await.is_err()
                || response_line.is_empty()
            {
                *guard = None;
                if attempt == 0 {
                    continue;
                }
                bail!("bridge read failed after reconnect");
            }

            let parsed: Value = serde_json::from_str(response_line.trim())
                .context("bridge response was not valid JSON")?;

            if let Some(ok) = parsed.get("ok") {
                return Ok(ok.clone());
            }
            if let Some(err) = parsed.get("err") {
                bail!(
                    "bridge returned error: {}",
                    err.as_str().unwrap_or("unspecified")
                );
            }
            bail!("bridge response missing both `ok` and `err`: {}", parsed);
        }

        bail!("bridge unreachable")
    }
}
