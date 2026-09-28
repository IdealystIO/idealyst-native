//! Robot-bridge client — the single network mode the Inspector speaks.
//!
//! A background `std::thread` owns the blocking `TcpStream` to the target
//! app and the newline-JSON request/response loop (`{id,cmd,args}` ⇄
//! `{id,ok|err}`, see `runtime_shared::robot::bridge`). It can't block the
//! UI run loop, hence the thread. Each refresh re-issues the read verbs
//! and stores the parsed result in an `Arc<Mutex<Snapshot>>`; the UI
//! thread copies that into a signal on its own cadence.
//!
//! A SECOND connection `subscribe`s and turns each `{"event":"changed"}`
//! push into an immediate refresh, so the tree follows the app within
//! tens of milliseconds instead of on the fallback cadence.
//!
//! **Focus.** The per-item verbs (`get_component`, the root element's
//! frame and native read-back, `get_signal_history`) run only for what the
//! user has selected — [`BridgeClient::set_focus`] — so a refresh costs
//! the same whether the app has ten components or ten thousand.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use super::model::{
    ActionResult, ComponentDetail, ElementDetail, ElementNode, LogRow, Navigator, NativeNode, Perf,
    PhaseRow, Rect, SignalHistory, SignalRow, Snapshot, Status,
};

/// Fallback refresh cadence for state no push covers (signal values,
/// phase timers). Pushes refresh sooner.
const REFRESH_MS: u64 = 500;
/// Per-request read timeout. A live bridge replies in <50 ms; this only
/// bounds the wait on an unresponsive target (a suspended background app)
/// so the UI can say so instead of hanging.
const READ_TIMEOUT: Duration = Duration::from_secs(8);
/// Backoff before retrying a failed connection.
const RECONNECT_BACKOFF: Duration = Duration::from_millis(800);
/// Log lines pulled per refresh.
const LOG_LIMIT: u64 = 300;

/// What the per-item verbs fetch (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Focus {
    pub component: Option<u64>,
    pub signal: Option<u64>,
}

enum ClientMsg {
    Action { label: String, cmd: String, args: Value },
    Refresh,
}

/// A handle to a connected target. Dropping it stops the background
/// threads.
pub struct BridgeClient {
    addr: String,
    shared: Arc<Mutex<Snapshot>>,
    focus: Arc<Mutex<Focus>>,
    msgs: mpsc::Sender<ClientMsg>,
    stop: Arc<AtomicBool>,
}

impl Drop for BridgeClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl BridgeClient {
    /// Connect to `addr` (`127.0.0.1:53817`) and start the background
    /// loops. Returns immediately; the first snapshot lands once the main
    /// loop connects.
    pub fn connect(addr: String) -> Self {
        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let focus = Arc::new(Mutex::new(Focus::default()));
        let (tx, rx) = mpsc::channel::<ClientMsg>();
        let stop = Arc::new(AtomicBool::new(false));
        // At most one push-triggered refresh in flight: a burst of pushes
        // is one refresh.
        let refresh_pending = Arc::new(AtomicBool::new(false));
        {
            let (shared, focus, stop, pending, addr) =
                (shared.clone(), focus.clone(), stop.clone(), refresh_pending.clone(), addr.clone());
            std::thread::spawn(move || run_loop(addr, shared, focus, rx, stop, pending));
        }
        {
            let (tx, stop, addr) = (tx.clone(), stop.clone(), addr.clone());
            std::thread::spawn(move || push_listener(addr, tx, stop, refresh_pending));
        }
        Self { addr, shared, focus, msgs: tx, stop }
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// A clone of the latest state.
    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Change what the per-item verbs fetch, and refresh now so the
    /// detail pane fills without waiting for the next cadence.
    pub fn set_focus(&self, focus: Focus) {
        let changed = self.focus.lock().map(|mut f| std::mem::replace(&mut *f, focus) != focus).unwrap_or(false);
        if changed {
            let _ = self.msgs.send(ClientMsg::Refresh);
        }
    }

    /// Send an action verb; its outcome lands in `Snapshot::last_action`
    /// under `label`, followed by a refresh.
    pub fn action(&self, label: impl Into<String>, cmd: &str, args: Value) {
        let _ = self.msgs.send(ClientMsg::Action { label: label.into(), cmd: cmd.to_string(), args });
    }
}

fn push_listener(
    addr: String,
    msgs: mpsc::Sender<ClientMsg>,
    stop: Arc<AtomicBool>,
    refresh_pending: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        let Ok(stream) = TcpStream::connect(&addr) else {
            std::thread::sleep(RECONNECT_BACKOFF);
            continue;
        };
        // A finite timeout so `stop` is noticed between pushes; partial
        // bytes survive it in the BufReader.
        let _ = stream.set_read_timeout(Some(Duration::from_millis(1000)));
        let Ok(mut writer) = stream.try_clone() else { continue };
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
                Ok(0) => break,
                Ok(_) => {
                    let is_change = serde_json::from_str::<Value>(line.trim())
                        .ok()
                        .is_some_and(|v| v.get("event").and_then(|e| e.as_str()) == Some("changed"));
                    if is_change
                        && !refresh_pending.swap(true, Ordering::AcqRel)
                        && msgs.send(ClientMsg::Refresh).is_err()
                    {
                        return;
                    }
                }
                Err(ref e)
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
                {
                    continue
                }
                Err(_) => break,
            }
        }
        std::thread::sleep(RECONNECT_BACKOFF);
    }
}

/// One open request/response connection.
struct Conn {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
    next_id: u64,
}

impl Conn {
    fn open(addr: &str) -> Result<Conn, String> {
        let stream = TcpStream::connect(addr).map_err(|e| format!("connecting to {addr}: {e}"))?;
        let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
        let writer = stream.try_clone().map_err(|e| format!("socket clone: {e}"))?;
        Ok(Conn { writer, reader: BufReader::new(stream), next_id: 1 })
    }

    /// One round trip. `Err(Io)` means the connection is gone; `Err(Verb)`
    /// is the bridge refusing this one command.
    fn call(&mut self, cmd: &str, args: Value) -> Result<Value, CallError> {
        let id = self.next_id;
        self.next_id += 1;
        let line = format!("{}\n", json!({ "id": id, "cmd": cmd, "args": args }));
        self.writer
            .write_all(line.as_bytes())
            .and_then(|_| self.writer.flush())
            .map_err(|e| CallError::Io(format!("write failed: {e}")))?;
        let mut resp = String::new();
        let n = self.reader.read_line(&mut resp).map_err(|e| {
            if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
                CallError::Io(
                    "the app is not responding. A macOS app in the background can be suspended \
                     by the OS; bring its window to the front."
                        .to_string(),
                )
            } else {
                CallError::Io(format!("read failed: {e}"))
            }
        })?;
        if n == 0 {
            return Err(CallError::Io("connection closed".into()));
        }
        let v: Value = serde_json::from_str(resp.trim()).map_err(|e| CallError::Io(format!("bad reply: {e}")))?;
        if let Some(ok) = v.get("ok") {
            Ok(ok.clone())
        } else {
            Err(CallError::Verb(v.get("err").and_then(|e| e.as_str()).unwrap_or("unspecified error").to_string()))
        }
    }

    /// A call whose reply parses as `T`; a verb error or a shape mismatch
    /// is an `Ok(Err)` (this command failed, the connection is fine).
    fn call_as<T: DeserializeOwned>(&mut self, cmd: &str, args: Value) -> Result<Result<T, String>, String> {
        match self.call(cmd, args) {
            Ok(v) => Ok(serde_json::from_value(v).map_err(|e| format!("{cmd}: unexpected reply: {e}"))),
            Err(CallError::Verb(e)) => Ok(Err(e)),
            Err(CallError::Io(e)) => Err(e),
        }
    }
}

enum CallError {
    Io(String),
    Verb(String),
}

fn run_loop(
    addr: String,
    shared: Arc<Mutex<Snapshot>>,
    focus: Arc<Mutex<Focus>>,
    msgs: mpsc::Receiver<ClientMsg>,
    stop: Arc<AtomicBool>,
    refresh_pending: Arc<AtomicBool>,
) {
    let mut last_action: Option<ActionResult> = None;
    // `get_perf_counters` DRAINS the target's counters on every read, so a
    // refresh sees only the interval since the previous one. Summed here
    // they read "since connect" (or since the user's Reset).
    let mut perf_acc: std::collections::BTreeMap<String, PhaseRow> = Default::default();
    while !stop.load(Ordering::Relaxed) {
        let mut conn = match Conn::open(&addr) {
            Ok(c) => c,
            Err(e) => {
                set_down(&shared, e);
                std::thread::sleep(RECONNECT_BACKOFF);
                continue;
            }
        };
        'session: loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let f = focus.lock().map(|f| *f).unwrap_or_default();
            match refresh(&mut conn, f) {
                Ok(mut snap) => {
                    snap.last_action = last_action.clone();
                    if let Perf::Rows(interval) = &snap.perf {
                        for row in interval {
                            let acc = perf_acc.entry(row.phase.clone()).or_insert_with(|| PhaseRow {
                                phase: row.phase.clone(),
                                call_count: 0,
                                total_us: 0,
                                max_us: 0,
                            });
                            acc.call_count += row.call_count;
                            acc.total_us += row.total_us;
                            acc.max_us = acc.max_us.max(row.max_us);
                        }
                        snap.perf = Perf::Rows(perf_acc.values().cloned().collect());
                    }
                    if let Ok(mut g) = shared.lock() {
                        *g = snap;
                    }
                }
                Err(e) => {
                    set_down(&shared, e);
                    break;
                }
            }
            loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                match msgs.recv_timeout(Duration::from_millis(REFRESH_MS)) {
                    Ok(ClientMsg::Action { label, cmd, args }) => {
                        if cmd == "clear_perf_counters" {
                            perf_acc.clear();
                        }
                        let started = Instant::now();
                        let result = match conn.call(&cmd, args) {
                            Ok(_) => Ok(()),
                            Err(CallError::Verb(e)) => Err(e),
                            Err(CallError::Io(e)) => {
                                set_down(&shared, e);
                                break 'session;
                            }
                        };
                        last_action =
                            Some(ActionResult { label, result, rtt_ms: started.elapsed().as_millis() as u64 });
                        break; // refresh so the change shows
                    }
                    Ok(ClientMsg::Refresh) => {
                        refresh_pending.store(false, Ordering::Release);
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        }
        std::thread::sleep(RECONNECT_BACKOFF);
    }
}

/// Pull every read surface into a fresh snapshot. A connection failure
/// propagates (reconnect); a single verb the target doesn't serve just
/// leaves its part empty — except `get_snapshot`. The tree is what the
/// Inspector is for, and the bridge refusing it means there is no app to
/// read: the CLI's relay answers "no app connected to the relay" until
/// the app dials in. Defaulting that to an empty tree showed a Live
/// status over a blank hierarchy; it reads as Not connected instead.
fn refresh(conn: &mut Conn, focus: Focus) -> Result<Snapshot, String> {
    let started = Instant::now();
    let tree: Vec<ElementNode> =
        conn.call_as("get_snapshot", json!({}))?.map_err(|e| format!("get_snapshot: {e}"))?;
    let rtt_ms = started.elapsed().as_millis() as u64;
    let component_count =
        conn.call_as::<Vec<Value>>("list_components", json!({}))?.map(|v| v.len()).unwrap_or(0);
    let signals: Vec<SignalRow> = conn.call_as("list_watched_signals", json!({}))?.unwrap_or_default();
    let navigators: Vec<Navigator> = conn.call_as("list_navigators", json!({}))?.unwrap_or_default();
    let logs: Vec<LogRow> = conn.call_as("get_logs", json!({ "limit": LOG_LIMIT }))?.unwrap_or_default();
    let perf = match conn.call_as::<Vec<PhaseRow>>("get_perf_counters", json!({}))? {
        Ok(rows) => Perf::Rows(rows),
        Err(hint) => Perf::Unavailable(hint),
    };

    let component = match focus.component {
        Some(id) => conn.call_as::<Option<ComponentDetail>>("get_component", json!({ "instance_id": id }))?.ok().flatten(),
        None => None,
    };
    let element = match component.as_ref().and_then(|c| c.element_id) {
        Some(element_id) => {
            let args = json!({ "element_id": element_id });
            let frame = conn.call_as::<Option<Rect>>("get_absolute_frame", args.clone())?.ok().flatten();
            let native = conn.call_as::<Option<NativeNode>>("introspect_native", args)?.ok().flatten();
            Some(ElementDetail { element_id, frame, native })
        }
        None => None,
    };
    let signal_history = match focus.signal {
        Some(id) => conn.call_as::<Option<SignalHistory>>("get_signal_history", json!({ "id": id }))?.ok().flatten(),
        None => None,
    };

    Ok(Snapshot {
        status: Status::Live { rtt_ms },
        tree,
        component_count,
        component,
        element,
        signals,
        signal_history,
        navigators,
        logs,
        perf,
        last_action: None,
    })
}

fn set_down(shared: &Arc<Mutex<Snapshot>>, msg: String) {
    if let Ok(mut g) = shared.lock() {
        g.status = Status::Down(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A bridge stand-in that refuses every verb with `err`, as the CLI's
    /// relay does while no app is connected.
    fn refusing_bridge(err: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut writer = stream.try_clone().expect("clone");
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { return };
                let id = serde_json::from_str::<Value>(&line).map(|v| v["id"].clone()).unwrap_or(Value::Null);
                let reply = format!("{}\n", json!({ "id": id, "err": err }));
                if writer.write_all(reply.as_bytes()).is_err() {
                    return;
                }
            }
        });
        addr
    }

    #[test]
    fn regression_relay_without_an_app_reads_as_not_connected() {
        let addr = refusing_bridge("no app connected to the relay");
        let mut conn = Conn::open(&addr).expect("open");
        let err = refresh(&mut conn, Focus::default()).expect_err(
            "a refused get_snapshot must fail the refresh, not render an empty tree as Live",
        );
        assert!(err.contains("no app connected to the relay"), "{err}");
    }
}
