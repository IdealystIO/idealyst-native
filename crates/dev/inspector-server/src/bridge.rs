//! One request/response connection to an app's robot bridge (the TCP
//! newline-JSON protocol, `{id,cmd,args}` ⇄ `{id,ok|err}`; see
//! `runtime_shared::robot::bridge`), and the reads a refresh issues.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use inspector_protocol::{
    ComponentDetail, ElementDetail, ElementNode, Focus, LogRow, Navigator, NativeNode, PhaseRow, Rect, SignalHistory,
    SignalRow,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

/// Per-request read timeout. A live bridge replies in <50 ms; this only
/// bounds the wait on an unresponsive target (a suspended background app)
/// so the front end can say so instead of hanging.
const READ_TIMEOUT: Duration = Duration::from_secs(8);
/// Log lines pulled per refresh.
const LOG_LIMIT: u64 = 300;

pub(crate) struct Conn {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
    next_id: u64,
}

pub(crate) enum CallError {
    /// The connection is gone.
    Io(String),
    /// The bridge refused this one command; the connection is fine.
    Verb(String),
}

impl Conn {
    pub(crate) fn open(addr: &str) -> Result<Conn, String> {
        let stream = TcpStream::connect(addr).map_err(|e| format!("connecting to {addr}: {e}"))?;
        let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
        let writer = stream.try_clone().map_err(|e| format!("socket clone: {e}"))?;
        Ok(Conn { writer, reader: BufReader::new(stream), next_id: 1 })
    }

    pub(crate) fn call(&mut self, cmd: &str, args: Value) -> Result<Value, CallError> {
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

/// The part of a refresh every front end attached to the app shares.
#[derive(Clone, Debug)]
pub(crate) struct Base {
    pub rtt_ms: u64,
    pub tree: Vec<ElementNode>,
    pub component_count: usize,
    pub signals: Vec<SignalRow>,
    pub navigators: Vec<Navigator>,
    pub logs: Vec<LogRow>,
    /// This interval's phase timers (the bridge DRAINS them on read), or
    /// the bridge's reason there are none.
    pub perf: Result<Vec<PhaseRow>, String>,
}

/// The part of a refresh that follows one front end's [`Focus`].
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Detail {
    pub component: Option<ComponentDetail>,
    pub element: Option<ElementDetail>,
    pub signal_history: Option<SignalHistory>,
}

/// Read the shared surfaces. A connection failure propagates (reconnect);
/// a single verb the target doesn't serve just leaves its part empty —
/// except `get_snapshot`. The tree is what the Inspector is for, and the
/// bridge refusing it means there is no app to read: the CLI's relay
/// answers "no app connected to the relay" until the app dials in.
/// Defaulting that to an empty tree showed a Live status over a blank
/// hierarchy; it reads as Not connected instead.
pub(crate) fn read_base(conn: &mut Conn) -> Result<Base, String> {
    let started = Instant::now();
    let tree: Vec<ElementNode> =
        conn.call_as("get_snapshot", json!({}))?.map_err(|e| format!("get_snapshot: {e}"))?;
    let rtt_ms = started.elapsed().as_millis() as u64;
    let component_count =
        conn.call_as::<Vec<Value>>("list_components", json!({}))?.map(|v| v.len()).unwrap_or(0);
    let signals = conn.call_as("list_watched_signals", json!({}))?.unwrap_or_default();
    let navigators = conn.call_as("list_navigators", json!({}))?.unwrap_or_default();
    let logs = conn.call_as("get_logs", json!({ "limit": LOG_LIMIT }))?.unwrap_or_default();
    let perf = conn.call_as::<Vec<PhaseRow>>("get_perf_counters", json!({}))?;
    Ok(Base { rtt_ms, tree, component_count, signals, navigators, logs, perf })
}

/// Read what `focus` selects. Costs nothing when nothing is selected, so
/// a refresh costs the same whether the app has ten components or ten
/// thousand.
pub(crate) fn read_detail(conn: &mut Conn, focus: Focus) -> Result<Detail, String> {
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
    Ok(Detail { component, element, signal_history })
}
