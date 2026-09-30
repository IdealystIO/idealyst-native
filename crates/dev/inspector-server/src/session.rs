//! One inspected app: the single robot-bridge connection every front end
//! attached to it shares.
//!
//! The session thread owns the connection and a refresh loop. Each
//! refresh reads the shared surfaces once ([`bridge::read_base`]), then
//! the per-item detail once per distinct [`Focus`] among the attached
//! front ends, and pushes each front end its own [`Snapshot`] — only when
//! it differs from the last one that front end was sent.
//!
//! Sharing is not just economy. `get_perf_counters` DRAINS the app's
//! counters, so two front ends polling one app separately would each see
//! a fraction of the calls. Here the one connection drains them and the
//! session accumulates them, so every front end reads the same "since
//! attach" totals.
//!
//! A SECOND connection `subscribe`s to the bridge's change pushes and
//! turns each into an immediate refresh, so the tree follows the app
//! within tens of milliseconds; a fallback cadence covers the state no
//! push tracks (signal values, phase timers).

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use inspector_protocol::{ActionResult, Focus, Perf, PhaseRow, ServerMsg, Snapshot, Status};
use serde_json::Value;

use crate::bridge::{self, CallError, Conn, Detail};

/// Fallback refresh cadence for state no push covers.
const REFRESH_MS: u64 = 500;
/// Backoff before retrying a failed connection.
const RECONNECT_BACKOFF: Duration = Duration::from_millis(800);
/// How often the push listener wakes to notice the session ended.
const PUSH_STOP_POLL: Duration = Duration::from_millis(1000);

/// A front end, as the hub numbers them.
pub(crate) type TabId = u64;

/// Frames queued for one front end's socket.
pub(crate) type Outbound = Sender<String>;

pub(crate) enum SessionMsg {
    AddTab { tab: TabId, out: Outbound, focus: Focus },
    RemoveTab(TabId),
    Focus(TabId, Focus),
    Action { tab: TabId, label: String, cmd: String, args: Value },
    /// Box an element in the app (`None` clears). Fire-and-forget.
    Highlight(TabId, Option<u64>),
    /// The app pushed a change.
    Refresh,
}

/// Live sessions by key, shared with the hub. A session removes its own
/// entry (under this lock) when its last front end leaves; see
/// [`Session::try_retire`].
pub(crate) type Registry = Arc<Mutex<HashMap<String, Sender<SessionMsg>>>>;

struct Tab {
    out: Outbound,
    focus: Focus,
    last_action: Option<ActionResult>,
    /// The last snapshot this front end was sent, and its encoding.
    last: Snapshot,
    last_sent: Option<String>,
}

pub(crate) struct Session {
    /// The registry key.
    key: String,
    /// What front ends attached with; echoed in every snapshot frame.
    app: String,
    addr: String,
    tabs: HashMap<TabId, Tab>,
    /// Phase timers summed across refreshes (the bridge drains them).
    perf_acc: BTreeMap<String, PhaseRow>,
    rx: Receiver<SessionMsg>,
    /// The front end whose highlight is showing, so its leaving clears it.
    highlighted_by: Option<TabId>,
    registry: Registry,
    stop: Arc<AtomicBool>,
    refresh_pending: Arc<AtomicBool>,
}

enum Wake {
    Refresh,
    ConnLost(String),
    Exit,
}

/// Start a session for the bridge at `addr`, registered under `key`. The
/// caller holds the registry lock and inserts the returned sender.
pub(crate) fn spawn(key: String, app: String, addr: String, registry: Registry) -> Sender<SessionMsg> {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let refresh_pending = Arc::new(AtomicBool::new(false));
    {
        let (addr, tx, stop, pending) = (addr.clone(), tx.clone(), stop.clone(), refresh_pending.clone());
        std::thread::spawn(move || push_listener(addr, tx, stop, pending));
    }
    let session = Session {
        key,
        app,
        addr,
        tabs: HashMap::new(),
        perf_acc: BTreeMap::new(),
        rx,
        highlighted_by: None,
        registry,
        stop,
        refresh_pending,
    };
    std::thread::spawn(move || session.run());
    tx
}

impl Session {
    fn run(mut self) {
        'connect: loop {
            let mut conn = match Conn::open(&self.addr) {
                Ok(c) => c,
                Err(e) => {
                    self.broadcast_down(&e);
                    match self.wait(RECONNECT_BACKOFF, None) {
                        Wake::Exit => break 'connect,
                        _ => continue 'connect,
                    }
                }
            };
            loop {
                if let Err(e) = self.refresh(&mut conn) {
                    self.broadcast_down(&e);
                    break;
                }
                match self.wait(Duration::from_millis(REFRESH_MS), Some(&mut conn)) {
                    Wake::Refresh => {}
                    Wake::ConnLost(e) => {
                        self.broadcast_down(&e);
                        break;
                    }
                    Wake::Exit => break 'connect,
                }
            }
            if let Wake::Exit = self.wait(RECONNECT_BACKOFF, None) {
                break;
            }
        }
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Block for the next reason to refresh, handling front-end traffic
    /// meanwhile. Actions run on `conn` when connected.
    fn wait(&mut self, timeout: Duration, mut conn: Option<&mut Conn>) -> Wake {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let msg = match self.rx.recv_timeout(left) {
                Ok(m) => m,
                Err(mpsc::RecvTimeoutError::Timeout) => return Wake::Refresh,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Wake::Exit,
            };
            let wake = self.handle(msg, conn.as_deref_mut());
            if self.tabs.is_empty() && self.try_retire() {
                return Wake::Exit;
            }
            if let Some(wake) = wake {
                return wake;
            }
        }
    }

    /// Apply one message. `Some` ends the wait.
    fn handle(&mut self, msg: SessionMsg, conn: Option<&mut Conn>) -> Option<Wake> {
        match msg {
            SessionMsg::AddTab { tab, out, focus } => {
                self.tabs.insert(tab, Tab { out, focus, last_action: None, last: Snapshot::default(), last_sent: None });
                Some(Wake::Refresh)
            }
            SessionMsg::RemoveTab(tab) => {
                self.tabs.remove(&tab);
                // A front end that closes mid-hover must not leave its box
                // painted over the app.
                if self.highlighted_by == Some(tab) {
                    self.highlighted_by = None;
                    if let Some(conn) = conn {
                        let _ = conn.call("clear_highlight", serde_json::json!({}));
                    }
                }
                None
            }
            SessionMsg::Highlight(tab, element) => {
                let Some(conn) = conn else { return None };
                // The app may lack a frame for the element, or be an older
                // build without the verb: a missing box is the whole cost.
                let result = match element {
                    Some(id) => conn.call("highlight_element", serde_json::json!({ "element_id": id })),
                    None => conn.call("clear_highlight", serde_json::json!({})),
                };
                self.highlighted_by = element.map(|_| tab);
                match result {
                    Err(CallError::Io(e)) => Some(Wake::ConnLost(e)),
                    _ => None,
                }
            }
            SessionMsg::Focus(tab, focus) => {
                let t = self.tabs.get_mut(&tab)?;
                (t.focus != focus).then(|| {
                    t.focus = focus;
                    Wake::Refresh
                })
            }
            SessionMsg::Action { tab, label, cmd, args } => {
                if cmd == "clear_perf_counters" {
                    self.perf_acc.clear();
                }
                let started = Instant::now();
                let (result, lost) = match conn {
                    Some(conn) => match conn.call(&cmd, args) {
                        Ok(_) => (Ok(()), None),
                        Err(CallError::Verb(e)) => (Err(e), None),
                        Err(CallError::Io(e)) => (Err(e.clone()), Some(e)),
                    },
                    None => (Err("the app is not connected".to_string()), None),
                };
                if let Some(t) = self.tabs.get_mut(&tab) {
                    t.last_action = Some(ActionResult { label, result, rtt_ms: started.elapsed().as_millis() as u64 });
                }
                Some(lost.map(Wake::ConnLost).unwrap_or(Wake::Refresh))
            }
            SessionMsg::Refresh => {
                self.refresh_pending.store(false, Ordering::Release);
                Some(Wake::Refresh)
            }
        }
    }

    /// With no front ends left, leave the registry — unless one attached
    /// meanwhile. Attaching sends under the registry lock, so draining the
    /// channel while holding it sees every attach that could still land.
    fn try_retire(&mut self) -> bool {
        let registry = self.registry.clone();
        let mut reg = registry.lock().unwrap();
        while let Ok(msg) = self.rx.try_recv() {
            let _ = self.handle(msg, None);
        }
        if self.tabs.is_empty() {
            reg.remove(&self.key);
            true
        } else {
            false
        }
    }

    fn refresh(&mut self, conn: &mut Conn) -> Result<(), String> {
        let base = bridge::read_base(conn)?;
        let perf = match base.perf.clone() {
            Ok(interval) => {
                for row in interval {
                    let acc = self.perf_acc.entry(row.phase.clone()).or_insert_with(|| PhaseRow {
                        phase: row.phase.clone(),
                        call_count: 0,
                        total_us: 0,
                        max_us: 0,
                    });
                    acc.call_count += row.call_count;
                    acc.total_us += row.total_us;
                    acc.max_us = acc.max_us.max(row.max_us);
                }
                Perf::Rows(self.perf_acc.values().cloned().collect())
            }
            Err(hint) => Perf::Unavailable(hint),
        };
        let mut details: HashMap<Focus, Detail> = HashMap::new();
        let ids: Vec<TabId> = self.tabs.keys().copied().collect();
        for id in ids {
            let focus = self.tabs[&id].focus;
            if !details.contains_key(&focus) {
                details.insert(focus, bridge::read_detail(conn, focus)?);
            }
            let detail = details[&focus].clone();
            let tab = self.tabs.get_mut(&id).expect("tab listed above");
            let snap = Snapshot {
                status: Status::Live { rtt_ms: base.rtt_ms },
                tree: base.tree.clone(),
                component_count: base.component_count,
                component: detail.component,
                element: detail.element,
                signals: base.signals.clone(),
                signal_history: detail.signal_history,
                navigators: base.navigators.clone(),
                logs: base.logs.clone(),
                perf: perf.clone(),
                last_action: tab.last_action.clone(),
            };
            send_snapshot(&self.app, tab, snap);
        }
        Ok(())
    }

    /// Every front end keeps what it last saw, marked Not connected.
    fn broadcast_down(&mut self, err: &str) {
        for tab in self.tabs.values_mut() {
            let mut snap = tab.last.clone();
            snap.status = Status::Down(err.to_string());
            snap.last_action = tab.last_action.clone();
            send_snapshot(&self.app, tab, snap);
        }
    }
}

/// Send `snap` unless it's what this front end already has. A closed
/// socket is ignored: its front end's own thread removes the tab.
fn send_snapshot(app: &str, tab: &mut Tab, snap: Snapshot) {
    let frame = ServerMsg::Snapshot { app: app.to_string(), snapshot: Box::new(snap.clone()) }.to_json();
    if tab.last_sent.as_deref() != Some(frame.as_str()) {
        let _ = tab.out.send(frame.clone());
        tab.last_sent = Some(frame);
    }
    tab.last = snap;
}

/// Hold a `subscribe`d connection and turn each `changed` push into one
/// [`SessionMsg::Refresh`] — at most one in flight, so a burst of pushes
/// is one refresh.
fn push_listener(addr: String, msgs: Sender<SessionMsg>, stop: Arc<AtomicBool>, refresh_pending: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        let Ok(stream) = TcpStream::connect(&addr) else {
            std::thread::sleep(RECONNECT_BACKOFF);
            continue;
        };
        // A finite timeout so `stop` is noticed between pushes; partial
        // bytes survive it in the BufReader.
        let _ = stream.set_read_timeout(Some(PUSH_STOP_POLL));
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
                        && msgs.send(SessionMsg::Refresh).is_err()
                    {
                        return;
                    }
                }
                Err(ref e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    continue
                }
                Err(_) => break,
            }
        }
        std::thread::sleep(RECONNECT_BACKOFF);
    }
}
