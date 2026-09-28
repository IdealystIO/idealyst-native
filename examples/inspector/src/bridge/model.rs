//! The target app's state as the Inspector shows it: typed mirrors of the
//! robot bridge's JSON replies, plus the pure derivations the screens
//! render (the component tree's visible rows, a component's breadcrumb).
//!
//! No UI and no transport in here — the same model serves any front end
//! that can run a bridge client (this desktop app today; a CLI-hosted
//! view later).

use std::collections::HashSet;

use serde::Deserialize;

// =============================================================================
// Wire mirrors (field names match the bridge's JSON exactly)
// =============================================================================

/// One `get_snapshot` node.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ComponentRef {
    pub instance_id: u64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct MethodArg {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
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
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Deserialize)]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One canonical native property (`introspect_native`'s `{type,value}`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
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
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct NativeNode {
    pub class: String,
    #[serde(default)]
    pub props: std::collections::BTreeMap<String, NativeValue>,
}

/// The selected component's root element, as the host drew it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ElementDetail {
    pub element_id: u64,
    pub frame: Option<Rect>,
    pub native: Option<NativeNode>,
}

/// `list_watched_signals`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct HistoryPoint {
    #[serde(default)]
    pub ago_ms: Option<u64>,
    pub value: String,
}

/// `get_signal_history`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct SignalHistory {
    pub id: u64,
    pub name: String,
    pub writes: u64,
    pub history: Vec<HistoryPoint>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct StackEntry {
    pub route: String,
    pub path: String,
}

/// `list_navigators`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct LogRow {
    /// Milliseconds since the Unix epoch.
    pub ts: u64,
    pub source: String,
    pub text: String,
}

/// `get_perf_counters`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
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
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Connecting,
    /// Connected; the last refresh's round trip in milliseconds.
    Live { rtt_ms: u64 },
    /// Not connected (the client keeps retrying).
    Down(String),
}

/// The outcome of the last action the user triggered (invoke, write,
/// navigate), shown next to the control that sent it.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionResult {
    pub label: String,
    pub result: Result<(), String>,
    pub rtt_ms: u64,
}

/// Everything the Inspector knows about the target. Plain data: it
/// crosses from the client thread to the UI thread by value.
#[derive(Clone, Debug, Default, PartialEq)]
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
#[derive(Clone, Debug, PartialEq)]
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

// =============================================================================
// Derivations the screens render
// =============================================================================

/// Identity of a tree row across refreshes (what expand state keys on).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RowKey {
    Component(u64),
    Element(u64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    Component { instance_id: u64, name: String },
    Element { id: u64, kind: String },
}

/// One visible line of the component tree.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeRow {
    pub key: RowKey,
    pub depth: usize,
    pub kind: RowKind,
    /// Secondary text: a component's root `test_id`, an element's
    /// `#id · test_id "label"`.
    pub meta: String,
    pub has_children: bool,
    pub expanded: bool,
    /// The component this row selects: itself, or for an element row the
    /// innermost component it belongs to (`None` above every component).
    pub owner: Option<u64>,
}

impl Default for TreeRow {
    fn default() -> Self {
        TreeRow {
            key: RowKey::Element(0),
            depth: 0,
            kind: RowKind::Element { id: 0, kind: String::new() },
            meta: String::new(),
            has_children: false,
            expanded: false,
            owner: None,
        }
    }
}

/// What the tree shows.
pub struct TreeOptions<'a> {
    /// Interleave the primitive elements under their components.
    pub show_elements: bool,
    /// Collapsed rows (everything else is open).
    pub collapsed: &'a HashSet<RowKey>,
    /// Case-insensitive name / `test_id` filter; matches keep their
    /// ancestors so the path to them stays visible.
    pub filter: &'a str,
}

/// An intermediate tree of display nodes: components nested by the
/// rendered hierarchy (a node's `components` field), elements optional.
struct DisplayNode {
    key: RowKey,
    kind: RowKind,
    meta: String,
    search: String,
    children: Vec<DisplayNode>,
}

fn element_meta(e: &ElementNode) -> String {
    let mut s = format!("#{}", e.id);
    if let Some(t) = &e.test_id {
        s.push_str(&format!(" · {t}"));
    }
    if let Some(l) = e.label.as_deref().filter(|l| !l.is_empty()) {
        s.push_str(&format!(" “{}”", truncate(l, 40)));
    }
    s
}

/// Build display nodes for `nodes` (siblings under one parent).
fn display_nodes(nodes: &[ElementNode], show_elements: bool) -> Vec<DisplayNode> {
    let mut out = Vec::new();
    for e in nodes {
        let mut children = display_nodes(&e.children, show_elements);
        if show_elements {
            children = vec![DisplayNode {
                key: RowKey::Element(e.id),
                kind: RowKind::Element { id: e.id, kind: e.kind.to_lowercase() },
                meta: element_meta(e),
                search: format!("{} {}", e.kind, e.test_id.as_deref().unwrap_or("")).to_lowercase(),
                children,
            }];
        }
        // Components wrap the element outermost-first: fold from the
        // innermost outwards so the outermost ends up on top.
        for c in e.components.iter().rev() {
            children = vec![DisplayNode {
                key: RowKey::Component(c.instance_id),
                kind: RowKind::Component { instance_id: c.instance_id, name: c.name.clone() },
                meta: e.test_id.clone().unwrap_or_default(),
                search: format!("{} {}", c.name, e.test_id.as_deref().unwrap_or("")).to_lowercase(),
                children,
            }];
        }
        out.extend(children);
    }
    out
}

/// Keep nodes that match (or have a matching descendant).
fn filter_nodes(nodes: Vec<DisplayNode>, needle: &str) -> Vec<DisplayNode> {
    nodes
        .into_iter()
        .filter_map(|mut n| {
            let kids = filter_nodes(std::mem::take(&mut n.children), needle);
            if n.search.contains(needle) || !kids.is_empty() {
                n.children = kids;
                Some(n)
            } else {
                None
            }
        })
        .collect()
}

fn flatten(
    nodes: &[DisplayNode],
    depth: usize,
    owner: Option<u64>,
    collapsed: &HashSet<RowKey>,
    force_open: bool,
    out: &mut Vec<TreeRow>,
) {
    for n in nodes {
        let expanded = force_open || !collapsed.contains(&n.key);
        let owner = match n.kind {
            RowKind::Component { instance_id, .. } => Some(instance_id),
            RowKind::Element { .. } => owner,
        };
        out.push(TreeRow {
            key: n.key,
            depth,
            kind: n.kind.clone(),
            meta: n.meta.clone(),
            has_children: !n.children.is_empty(),
            expanded,
            owner,
        });
        if expanded {
            flatten(&n.children, depth + 1, owner, collapsed, force_open, out);
        }
    }
}

/// The component tree's visible rows, top to bottom.
pub fn visible_rows(tree: &[ElementNode], opts: &TreeOptions) -> Vec<TreeRow> {
    let mut nodes = display_nodes(tree, opts.show_elements);
    let needle = opts.filter.trim().to_lowercase();
    let filtering = !needle.is_empty();
    if filtering {
        nodes = filter_nodes(nodes, &needle);
    }
    let mut out = Vec::new();
    // While filtering, every surviving branch is open: a match hidden
    // under a collapsed ancestor would read as "no results".
    flatten(&nodes, 0, None, opts.collapsed, filtering, &mut out);
    out
}

/// The component names from the root down to `instance_id`, inclusive.
pub fn component_path(tree: &[ElementNode], instance_id: u64) -> Vec<String> {
    fn walk(nodes: &[ElementNode], target: u64, trail: &mut Vec<String>) -> bool {
        for e in nodes {
            let pushed = e.components.len();
            for c in &e.components {
                trail.push(c.name.clone());
                if c.instance_id == target {
                    return true;
                }
            }
            if walk(&e.children, target, trail) {
                return true;
            }
            trail.truncate(trail.len() - pushed);
        }
        false
    }
    let mut trail = Vec::new();
    if walk(tree, instance_id, &mut trail) {
        trail
    } else {
        Vec::new()
    }
}

/// Count elements and component instances in the tree.
pub fn tree_counts(tree: &[ElementNode]) -> (usize, usize) {
    fn walk(nodes: &[ElementNode], acc: &mut (usize, usize)) {
        for e in nodes {
            acc.0 += e.components.len();
            acc.1 += 1;
            walk(&e.children, acc);
        }
    }
    let mut acc = (0, 0);
    walk(tree, &mut acc);
    acc
}

/// Find an element by id.
pub fn find_element(tree: &[ElementNode], id: u64) -> Option<&ElementNode> {
    for e in tree {
        if e.id == id {
            return Some(e);
        }
        if let Some(hit) = find_element(&e.children, id) {
            return Some(hit);
        }
    }
    None
}

/// `2.4 s ago`, `3 min ago`.
pub fn ago(ms: u64) -> String {
    match ms {
        0..=999 => "just now".to_string(),
        1_000..=59_999 => format!("{} s ago", ms / 1000),
        60_000..=3_599_999 => format!("{} min ago", ms / 60_000),
        _ => format!("{} h ago", ms / 3_600_000),
    }
}

/// Microseconds as `38.2 ms` / `412 µs`.
pub fn micros(us: u64) -> String {
    if us >= 1000 {
        format!("{} ms", trim_number(us as f64 / 1000.0))
    } else {
        format!("{us} µs")
    }
}

fn trim_number(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(id: u64, kind: &str, comps: &[(u64, &str)], children: Vec<ElementNode>) -> ElementNode {
        ElementNode {
            id,
            kind: kind.to_string(),
            test_id: None,
            label: None,
            components: comps.iter().map(|(i, n)| ComponentRef { instance_id: *i, name: n.to_string() }).collect(),
            children,
        }
    }

    /// App › Card › (Wrapper › Leaf) — Card's element holds a text whose
    /// element is shared by Wrapper and Leaf.
    fn sample() -> Vec<ElementNode> {
        vec![el(1, "View", &[(10, "App")], vec![el(2, "View", &[(11, "Card")], vec![el(3, "Text", &[(12, "Wrapper"), (13, "Leaf")], vec![])]), el(4, "Text", &[], vec![])])]
    }

    fn labels(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| {
                let name = match &r.kind {
                    RowKind::Component { name, .. } => name.clone(),
                    RowKind::Element { kind, .. } => kind.clone(),
                };
                format!("{}{}", "  ".repeat(r.depth), name)
            })
            .collect()
    }

    #[test]
    fn components_only_tree_nests_by_the_rendered_hierarchy() {
        let collapsed = HashSet::new();
        let rows = visible_rows(&sample(), &TreeOptions { show_elements: false, collapsed: &collapsed, filter: "" });
        assert_eq!(labels(&rows), ["App", "  Card", "    Wrapper", "      Leaf"]);
    }

    #[test]
    fn elements_interleave_under_their_components() {
        let collapsed = HashSet::new();
        let rows = visible_rows(&sample(), &TreeOptions { show_elements: true, collapsed: &collapsed, filter: "" });
        assert_eq!(
            labels(&rows),
            ["App", "  view", "    Card", "      view", "        Wrapper", "          Leaf", "            text", "    text"]
        );
        let owners: Vec<Option<u64>> = rows.iter().map(|r| r.owner).collect();
        assert_eq!(
            owners,
            [Some(10), Some(10), Some(11), Some(11), Some(12), Some(13), Some(13), Some(10)],
            "an element row selects the innermost component it belongs to"
        );
    }

    #[test]
    fn collapsing_hides_descendants_and_filter_reopens_the_path() {
        let collapsed: HashSet<RowKey> = [RowKey::Component(11)].into();
        let rows = visible_rows(&sample(), &TreeOptions { show_elements: false, collapsed: &collapsed, filter: "" });
        assert_eq!(labels(&rows), ["App", "  Card"]);
        assert!(!rows[1].expanded && rows[1].has_children);

        let rows = visible_rows(&sample(), &TreeOptions { show_elements: false, collapsed: &collapsed, filter: "leaf" });
        assert_eq!(labels(&rows), ["App", "  Card", "    Wrapper", "      Leaf"], "a match's ancestors stay visible");
    }

    #[test]
    fn breadcrumb_and_counts() {
        assert_eq!(component_path(&sample(), 13), ["App", "Card", "Wrapper", "Leaf"]);
        assert!(component_path(&sample(), 99).is_empty());
        assert_eq!(tree_counts(&sample()), (4, 4));
    }

    #[test]
    fn parses_the_bridge_shapes() {
        let tree: Vec<ElementNode> = serde_json::from_str(
            r#"[{"id":1,"kind":"View","test_id":null,"label":null,"components":[{"instance_id":3,"name":"Card"}],"children":[]}]"#,
        )
        .unwrap();
        assert_eq!(tree[0].components[0].name, "Card");
        let nav: Navigator = serde_json::from_str(
            r#"{"nav_id":0,"element_id":3,"type_name":"stack_navigator","active_route":"c","active_path":"/c?step=2&x","depth":2,"can_go_back":true,"is_current":true,"base":"","stack":[{"route":"h","path":"/"}],"controllable":true}"#,
        )
        .unwrap();
        assert_eq!(nav.kind_label(), "Stack");
        assert_eq!(nav.query(), [("step".to_string(), "2".to_string()), ("x".to_string(), String::new())]);
        let v: NativeValue = serde_json::from_str(r#"{"type":"color","value":[1.0,0.5,0.0,1.0]}"#).unwrap();
        assert_eq!(v.display(), "#ff8000");
    }

    #[test]
    fn formatting() {
        assert_eq!(ago(2_400), "2 s ago");
        assert_eq!(micros(38_200), "38.2 ms");
        assert_eq!(micros(412), "412 µs");
        assert_eq!(truncate("abcdef", 3), "abc…");
    }
}
