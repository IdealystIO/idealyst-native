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
#[derive(Clone, Default, PartialEq)]
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

/// What wakes the main session loop: a queued action to send, or a request
/// to refresh now (raised by the push listener when the target reports a
/// change). Both ride one channel so either wakes the loop immediately.
enum ClientMsg {
    Action(String, Value),
    Refresh,
}

/// A handle to a connected target. Dropping it stops the background threads.
pub struct BridgeClient {
    shared: Arc<Mutex<Snapshot>>,
    msgs: mpsc::Sender<ClientMsg>,
    stop: Arc<AtomicBool>,
}

impl Drop for BridgeClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl BridgeClient {
    /// Connect to `addr` (e.g. `127.0.0.1:9718`) and start the background
    /// loops. Returns immediately; the first snapshot fills in once the main
    /// loop connects.
    ///
    /// Two threads:
    /// - **main loop** — request/response: sends actions and runs the full
    ///   refresh. Waits on a channel, so an action or a push-triggered refresh
    ///   wakes it immediately (no fixed poll latency), with a slow fallback
    ///   refresh for state that doesn't bump the revision (signals/arena/perf).
    /// - **push listener** — a SECOND connection that `subscribe`s and reads
    ///   the bridge's `{"event":"changed"}` pushes, raising `Refresh` so the
    ///   tree/logs update live (~tens of ms) instead of on the poll cadence.
    ///   A separate connection keeps the main one a clean request/response
    ///   channel (no demuxing pushes from replies).
    pub fn connect(addr: String) -> Self {
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let (tx, rx) = mpsc::channel::<ClientMsg>();
        let stop = Arc::new(AtomicBool::new(false));
        // Coalesces pushes: at most one `Refresh` is in flight until the main
        // loop services it, so a burst of pushes is one refresh.
        let refresh_pending = Arc::new(AtomicBool::new(false));
        {
            let shared = shared.clone();
            let stop = stop.clone();
            let refresh_pending = refresh_pending.clone();
            let addr = addr.clone();
            std::thread::spawn(move || run_loop(addr, shared, rx, stop, refresh_pending));
        }
        {
            let stop = stop.clone();
            let tx = tx.clone();
            let refresh_pending = refresh_pending.clone();
            std::thread::spawn(move || push_listener(addr, tx, stop, refresh_pending));
        }
        Self { shared, msgs: tx, stop }
    }

    /// A clone of the latest state. Cheap enough to call every UI poll.
    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Queue an action verb for the background thread (fire-and-forget).
    /// Besides raw bridge verbs, the pseudo-verb `click_test_id`
    /// (`{"test_id": "..."}`) resolves the element then clicks it.
    pub fn action(&self, cmd: &str, args: Value) {
        let _ = self.msgs.send(ClientMsg::Action(cmd.to_string(), args));
    }
}

/// Push-listener: a dedicated connection that subscribes and turns each
/// `{"event":"changed"}` push into a coalesced `Refresh` on the main loop.
fn push_listener(
    addr: String,
    msgs: mpsc::Sender<ClientMsg>,
    stop: Arc<AtomicBool>,
    refresh_pending: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        let stream = match TcpStream::connect(&addr) {
            Ok(s) => s,
            Err(_) => {
                std::thread::sleep(RECONNECT_BACKOFF);
                continue;
            }
        };
        // A finite read timeout lets us notice `stop` between pushes (read_line
        // would otherwise block until a push or EOF). Partial bytes survive a
        // timeout in the BufReader, so a fragmented push still reassembles.
        let _ = stream.set_read_timeout(Some(Duration::from_millis(1000)));
        let mut writer = match stream.try_clone() {
            Ok(w) => w,
            Err(_) => continue,
        };
        let mut reader = BufReader::new(stream);
        if writer
            .write_all(b"{\"id\":1,\"cmd\":\"subscribe\",\"args\":{}}\n")
            .and_then(|_| writer.flush())
            .is_err()
        {
            continue;
        }

        let mut line = String::new();
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break, // EOF → reconnect
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                        if v.get("event").and_then(|e| e.as_str()) == Some("changed") {
                            // Coalesce: only enqueue a Refresh if one isn't
                            // already pending (the main loop clears it).
                            if !refresh_pending.swap(true, Ordering::AcqRel) {
                                if msgs.send(ClientMsg::Refresh).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
                Err(ref e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    // Timeout — loop to re-check `stop`. `read_line` keeps any
                    // partial bytes buffered, so reassembly survives.
                    continue;
                }
                Err(_) => break, // real error → reconnect
            }
        }
        std::thread::sleep(RECONNECT_BACKOFF);
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
    msgs: mpsc::Receiver<ClientMsg>,
    stop: Arc<AtomicBool>,
    refresh_pending: Arc<AtomicBool>,
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

        // Session loop: refresh state, then block on the message channel
        // until something wakes us — an action to send, a push-driven
        // `Refresh`, or the fallback timeout — then loop to refresh again.
        //
        // Blocking on the channel (vs a fixed `sleep`) means an action is
        // sent the instant it's queued, and a `{"event":"changed"}` push from
        // the listener triggers a refresh in ~tens of ms. The fallback timeout
        // still refreshes periodically for state that doesn't bump the
        // revision (signal values, arena/perf counters).
        'session: loop {
            if stop.load(Ordering::Relaxed) {
                return;
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

            // Service messages until a refresh is triggered.
            loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                match msgs.recv_timeout(Duration::from_millis(REFRESH_MS)) {
                    Ok(ClientMsg::Action(cmd, args)) => {
                        if run_action(&mut writer, &mut reader, &mut next_id, &cmd, args).is_err() {
                            break 'session; // IO error → reconnect
                        }
                        // loop for the next message
                    }
                    Ok(ClientMsg::Refresh) => {
                        // A push arrived — clear the coalescing flag and
                        // refresh now.
                        refresh_pending.store(false, Ordering::Release);
                        break;
                    }
                    // Fallback cadence — refresh state the revision doesn't track.
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
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
