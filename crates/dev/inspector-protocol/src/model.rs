//! The target app's state as the Inspector shows it: typed mirrors of the
//! robot bridge's JSON replies (field names match the bridge exactly), and
//! [`Snapshot`], the whole picture the server pushes to a front end.
//!
//! Every type round-trips through serde: the server parses them out of
//! bridge replies and re-serializes them onto the Inspector socket, and
//! the front end parses them back.

use serde::{Deserialize, Serialize};

// =============================================================================
// Wire mirrors (field names match the bridge's JSON exactly)
// =============================================================================

/// One `get_snapshot` node.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ElementNode {
    pub id: u64,
    pub kind: String,
    #[serde(default)]
    pub test_id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// Component instances rendered as this element, outermost first.
    #[serde(default)]
    pub components: Vec<ComponentRef>,
    #[serde(default)]
    pub children: Vec<ElementNode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComponentRef {
    pub instance_id: u64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MethodArg {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Method {
    pub name: String,
    #[serde(default)]
    pub args: Vec<MethodArg>,
}

impl Method {
    /// `bump_by(n: i32)`.
    pub fn signature(&self) -> String {
        let args: Vec<String> = self.args.iter().map(|a| format!("{}: {}", a.name, a.ty)).collect();
        format!("{}({})", self.name, args.join(", "))
    }
}

/// One prop from `get_component`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Prop {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    /// `static` / `live` / `signal` / `handler` / `children` / `value` /
    /// `opaque`.
    pub mode: String,
    #[serde(default)]
    pub value: Option<String>,
}

/// `get_component`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComponentDetail {
    pub instance_id: u64,
    pub name: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line: u32,
    #[serde(default)]
    pub element_id: Option<u64>,
    #[serde(default)]
    pub methods: Vec<Method>,
    #[serde(default)]
    pub props: Vec<Prop>,
}

impl ComponentDetail {
    /// `src/counter.rs:14`, or `None` for a hand-registered entry.
    pub fn location(&self) -> Option<String> {
        (!self.file.is_empty()).then(|| format!("{}:{}", self.file, self.line))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One canonical native property (`introspect_native`'s `{type,value}`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeValue {
    #[serde(rename = "type")]
    pub ty: String,
    pub value: serde_json::Value,
}

impl NativeValue {
    /// Display text: colors as `#rrggbb`/`#rrggbbaa`, numbers trimmed.
    pub fn display(&self) -> String {
        match (self.ty.as_str(), &self.value) {
            ("color", serde_json::Value::Array(c)) if c.len() == 4 => {
                let ch = |i: usize| (c[i].as_f64().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8;
                let a = ch(3);
                if a == 255 {
                    format!("#{:02x}{:02x}{:02x}", ch(0), ch(1), ch(2))
                } else {
                    format!("#{:02x}{:02x}{:02x}{:02x}", ch(0), ch(1), ch(2), a)
                }
            }
            (_, serde_json::Value::Number(n)) => trim_number(n.as_f64().unwrap_or(0.0)),
            (_, serde_json::Value::String(s)) => s.clone(),
            (_, other) => other.to_string(),
        }
    }
}

/// `introspect_native` (children dropped — the detail pane shows one node).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NativeNode {
    pub class: String,
    #[serde(default)]
    pub props: std::collections::BTreeMap<String, NativeValue>,
}

/// The selected component's root element, as the host drew it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ElementDetail {
    pub element_id: u64,
    pub frame: Option<Rect>,
    pub native: Option<NativeNode>,
}

/// `list_watched_signals`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SignalRow {
    pub id: u64,
    pub name: String,
    pub value: serde_json::Value,
    #[serde(default)]
    pub writes: u64,
    #[serde(default)]
    pub changed_ago_ms: Option<u64>,
    #[serde(default)]
    pub writable: bool,
}

impl SignalRow {
    /// The `Debug` rendering the bridge sends as a JSON string.
    pub fn value_text(&self) -> String {
        match &self.value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryPoint {
    #[serde(default)]
    pub ago_ms: Option<u64>,
    pub value: String,
}

/// `get_signal_history`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignalHistory {
    pub id: u64,
    pub name: String,
    pub writes: u64,
    pub history: Vec<HistoryPoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StackEntry {
    pub route: String,
    pub path: String,
}

/// `list_navigators`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Navigator {
    pub nav_id: u64,
    #[serde(default)]
    pub element_id: Option<u64>,
    pub type_name: String,
    pub active_route: String,
    pub active_path: String,
    pub depth: u64,
    pub can_go_back: bool,
    pub is_current: bool,
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub stack: Vec<StackEntry>,
    #[serde(default)]
    pub controllable: bool,
}

impl Navigator {
    /// `stack_navigator` → `Stack`, `swap_navigator` → `Swap`; an SDK's
    /// own presentation label (`TabNavigator`, …) passes through.
    pub fn kind_label(&self) -> String {
        let base = self.type_name.rsplit("::").next().unwrap_or(&self.type_name);
        let base = base.strip_suffix("_navigator").unwrap_or(base);
        let mut chars = base.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().chain(chars).collect(),
            None => base.to_string(),
        }
    }

    /// The current path's query parameters, in order.
    pub fn query(&self) -> Vec<(String, String)> {
        let Some((_, q)) = self.active_path.split_once('?') else { return Vec::new() };
        q.split('&')
            .filter(|kv| !kv.is_empty())
            .map(|kv| match kv.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => (kv.to_string(), String::new()),
            })
            .collect()
    }
}

/// `get_logs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogRow {
    /// Milliseconds since the Unix epoch.
    pub ts: u64,
    pub source: String,
    pub text: String,
}

/// `get_perf_counters`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PhaseRow {
    pub phase: String,
    pub call_count: u64,
    pub total_us: u64,
    pub max_us: u64,
}

// =============================================================================
// The whole picture
// =============================================================================

/// Where the connection stands.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Status {
    #[default]
    Connecting,
    /// Connected; the last refresh's round trip in milliseconds.
    Live { rtt_ms: u64 },
    /// Not connected, and why. The server keeps retrying the app; the
    /// front end keeps retrying the server.
    Down(String),
}

/// The outcome of the last action the user triggered (invoke, write,
/// navigate), shown next to the control that sent it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionResult {
    pub label: String,
    pub result: Result<(), String>,
    pub rtt_ms: u64,
}

/// Everything the Inspector knows about the target, as one value. The
/// server assembles it (the shared part once per app, the focused detail
/// per front end) and pushes it whole whenever it changes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub status: Status,
    pub tree: Vec<ElementNode>,
    pub component_count: usize,
    pub component: Option<ComponentDetail>,
    pub element: Option<ElementDetail>,
    pub signals: Vec<SignalRow>,
    pub signal_history: Option<SignalHistory>,
    pub navigators: Vec<Navigator>,
    pub logs: Vec<LogRow>,
    pub perf: Perf,
    pub last_action: Option<ActionResult>,
}

/// Phase timers, or why there are none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Perf {
    Rows(Vec<PhaseRow>),
    /// The bridge's hint (the target lacks `debug-stats`).
    Unavailable(String),
}

impl Default for Perf {
    fn default() -> Self {
        Perf::Rows(Vec::new())
    }
}

/// `3`, `2.5`: one decimal, dropped when it rounds away.
pub fn trim_number(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}
