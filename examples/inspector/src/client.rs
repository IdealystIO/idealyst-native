//! Robot-bridge client — the single network mode the inspector speaks.
//!
//! A background `std::thread` owns the blocking `TcpStream` to the target
//! app and the newline-JSON request/response loop (`{id,cmd,args}` ⇄
//! `{id,ok|err}`, see `runtime_core::robot::bridge`). It can't block the
//! UI run loop, hence the thread. Every ~refresh it re-issues the read
//! verbs and stores the parsed results into an `Arc<Mutex<Snapshot>>`; the
//! UI thread polls that snapshot into a reactive signal (see `lib.rs`).
//! Action verbs (click, invoke) are queued from the UI over an mpsc and
//! executed by the thread between refreshes.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

/// How often the background thread re-reads the target's state.
const REFRESH_MS: u64 = 500;
/// Per-request read timeout. Generous (a live bridge replies in <50ms) so
/// it never races a real reply, but finite so an unresponsive target
/// (backgrounded/suspended app) surfaces a status instead of hanging.
const READ_TIMEOUT: Duration = Duration::from_secs(8);
/// Backoff before retrying a failed connection.
const RECONNECT_BACKOFF: Duration = Duration::from_millis(800);
/// Cap on log lines pulled per refresh.
const LOG_LIMIT: u64 = 200;

/// The latest state read from the target app. Plain data (no signals), so
/// it crosses the thread boundary; the UI thread copies it into signals.
#[derive(Clone, Default)]
pub struct Snapshot {
    pub connected: bool,
    pub error: Option<String>,
    /// `list_navigators` rows.
    pub navigators: Vec<Value>,
    /// `get_snapshot` root nodes (recursive tree).
    pub tree: Vec<Value>,
    /// `find_all_elements` — the FLAT list of every registered element,
    /// independent of tree parent-linkage. Diagnostic: if this is large but
    /// `tree` is just the navigator, content is registered but not reachable
    /// from snapshot roots.
    pub elements: Vec<Value>,
    /// `list_components` rows.
    pub components: Vec<Value>,
    /// `get_arena_stats` object.
    pub arena: Option<Value>,
    /// `get_perf_counters` rows, or the disabled-feature hint in `perf_error`.
    pub perf: Vec<Value>,
    pub perf_error: Option<String>,
    /// `list_watched_signals` rows.
    pub signals: Vec<Value>,
    /// `get_logs` rows.
    pub logs: Vec<Value>,
}

/// A handle to a connected target. Dropping it stops the background thread.
pub struct BridgeClient {
    shared: Arc<Mutex<Snapshot>>,
    actions: mpsc::Sender<(String, Value)>,
    stop: Arc<AtomicBool>,
}

impl Drop for BridgeClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl BridgeClient {
    /// Connect to `addr` (e.g. `127.0.0.1:9718`) and start the background
    /// refresh/action loop. Returns immediately; the first snapshot fills
    /// in once the thread connects.
    pub fn connect(addr: String) -> Self {
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let (atx, arx) = mpsc::channel::<(String, Value)>();
        let stop = Arc::new(AtomicBool::new(false));
        {
            let shared = shared.clone();
            let stop = stop.clone();
            std::thread::spawn(move || run_loop(addr, shared, arx, stop));
        }
        Self { shared, actions: atx, stop }
    }

    /// A clone of the latest state. Cheap enough to call every UI poll.
    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Queue an action verb for the background thread (fire-and-forget).
    /// Besides raw bridge verbs, the pseudo-verb `click_test_id`
    /// (`{"test_id": "..."}`) resolves the element then clicks it.
    pub fn action(&self, cmd: &str, args: Value) {
        let _ = self.actions.send((cmd.to_string(), args));
    }
}

/// One bridge round-trip. Writes `{id,cmd,args}\n`, reads one reply line,
/// returns its `ok` payload or the `err` string.
fn call(
    writer: &mut TcpStream,
    reader: &mut BufReader<TcpStream>,
    next_id: &mut u64,
    cmd: &str,
    args: Value,
) -> Result<Value, String> {
    let id = *next_id;
    *next_id += 1;
    let req = json!({ "id": id, "cmd": cmd, "args": args });
    let line = format!("{}\n", req);
    writer
        .write_all(line.as_bytes())
        .and_then(|_| writer.flush())
        .map_err(|e| format!("write failed: {e}"))?;

    let mut resp = String::new();
    let n = reader.read_line(&mut resp).map_err(|e| {
        use std::io::ErrorKind::*;
        if matches!(e.kind(), WouldBlock | TimedOut) {
            format!(
                "target not responding (cmd '{cmd}'). If it's a macOS app in the \
                 background, the OS may be suspending it — bring its window to the front."
            )
        } else {
            format!("read failed: {e}")
        }
    })?;
    if n == 0 {
        return Err("connection closed".into());
    }
    let v: Value = serde_json::from_str(resp.trim()).map_err(|e| format!("bad json: {e}"))?;
    if let Some(ok) = v.get("ok") {
        Ok(ok.clone())
    } else if let Some(err) = v.get("err") {
        Err(err.as_str().unwrap_or("unspecified").to_string())
    } else {
        Err("reply missing ok/err".into())
    }
}

fn run_loop(
    addr: String,
    shared: Arc<Mutex<Snapshot>>,
    actions: mpsc::Receiver<(String, Value)>,
    stop: Arc<AtomicBool>,
) {
    let mut next_id: u64 = 1;
    while !stop.load(Ordering::Relaxed) {
        // (Re)connect.
        let stream = match TcpStream::connect(&addr) {
            Ok(s) => s,
            Err(e) => {
                set_error(&shared, format!("connecting to {addr}: {e}"));
                std::thread::sleep(RECONNECT_BACKOFF);
                continue;
            }
        };
        // Finite read timeout. A responsive bridge replies in <50ms, so 8s
        // never races a real reply — but it bounds the wait when the target
        // is unresponsive (e.g. macOS suspends/throttles a BACKGROUNDED app,
        // freezing its run loop + bridge poll). Without a timeout the read
        // blocks forever and the UI is stuck on "connecting"; with it we
        // surface a clear status and keep retrying.
        let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
        let writer = match stream.try_clone() {
            Ok(w) => w,
            Err(e) => {
                set_error(&shared, format!("socket clone: {e}"));
                std::thread::sleep(RECONNECT_BACKOFF);
                continue;
            }
        };
        let mut writer = writer;
        let mut reader = BufReader::new(stream);

        // Session loop: drain actions, refresh state, repeat until an IO
        // error drops us back to reconnect.
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            // Pending actions first, so a tap feels responsive.
            let mut io_failed = false;
            while let Ok((cmd, args)) = actions.try_recv() {
                if let Err(_) = run_action(&mut writer, &mut reader, &mut next_id, &cmd, args) {
                    io_failed = true;
                    break;
                }
            }
            if io_failed {
                break;
            }

            match refresh(&mut writer, &mut reader, &mut next_id) {
                Ok(snap) => {
                    if let Ok(mut g) = shared.lock() {
                        *g = snap;
                    }
                }
                Err(e) => {
                    set_error(&shared, e);
                    break; // reconnect
                }
            }
            std::thread::sleep(Duration::from_millis(REFRESH_MS));
        }
    }
}

/// Execute one action verb, including the `click_test_id` compound.
fn run_action(
    writer: &mut TcpStream,
    reader: &mut BufReader<TcpStream>,
    next_id: &mut u64,
    cmd: &str,
    args: Value,
) -> Result<(), String> {
    if cmd == "click_test_id" {
        let test_id = args["test_id"].as_str().unwrap_or("");
        let found = call(writer, reader, next_id, "find_element", json!({ "test_id": test_id }))?;
        if let Some(id) = found.get("id").and_then(|i| i.as_u64()) {
            call(writer, reader, next_id, "click", json!({ "element_id": id }))?;
        }
        return Ok(());
    }
    call(writer, reader, next_id, cmd, args).map(|_| ())
}

/// Pull every read surface into a fresh snapshot. A failure on a core verb
/// propagates (triggers reconnect); the perf verb's failure is captured as
/// a hint (it just means the target lacks `debug-stats`).
fn refresh(
    writer: &mut TcpStream,
    reader: &mut BufReader<TcpStream>,
    next_id: &mut u64,
) -> Result<Snapshot, String> {
    let arr = |v: Value| v.as_array().cloned().unwrap_or_default();

    let navigators = arr(call(writer, reader, next_id, "list_navigators", json!({}))?);
    let tree = arr(call(writer, reader, next_id, "get_snapshot", json!({}))?);
    let elements = arr(call(writer, reader, next_id, "find_all_elements", json!({}))?);
    let components = arr(call(writer, reader, next_id, "list_components", json!({}))?);
    let arena = call(writer, reader, next_id, "get_arena_stats", json!({})).ok();
    let signals = arr(call(writer, reader, next_id, "list_watched_signals", json!({}))?);
    let logs = arr(call(
        writer,
        reader,
        next_id,
        "get_logs",
        json!({ "limit": LOG_LIMIT }),
    )?);

    let (perf, perf_error) =
        match call(writer, reader, next_id, "get_perf_counters", json!({})) {
            Ok(v) => (v.as_array().cloned().unwrap_or_default(), None),
            Err(e) => (Vec::new(), Some(e)),
        };

    Ok(Snapshot {
        connected: true,
        error: None,
        navigators,
        tree,
        elements,
        components,
        arena,
        perf,
        perf_error,
        signals,
        logs,
    })
}

fn set_error(shared: &Arc<Mutex<Snapshot>>, msg: String) {
    if let Ok(mut g) = shared.lock() {
        g.connected = false;
        g.error = Some(msg);
    }
}
