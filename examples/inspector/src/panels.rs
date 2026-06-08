//! Inspector tab panels.
//!
//! - [`ElementsPanel`] — master/detail: a collapsible, selectable element
//!   tree on the left; the selected element's details on the right.
//! - [`LogsPanel`] — the filterable log stream.
//! - [`StatsPanel`] — read-only runtime internals (arena/perf/signals +
//!   a navigator summary).
//!
//! ## Reactivity model (easy to get wrong)
//!
//! A `#[component]` body runs **once** — it does NOT re-run when a signal
//! it reads changes. Live updates flow only through reactive boundaries:
//!
//! - **Reactive list** — a `ui!` `for x in SIG, key = …` over a
//!   `Signal<Vec<_>>` rebuilds its rows (keyed) when `SIG` changes.
//! - **Reactive `text`** — inside a `#[component]` body, the macro rewrites
//!   `text(<expr containing .get()>)` into a reactive closure. Pass the
//!   *expression*, not a `move || …` closure (that double-wraps and won't
//!   typecheck).
//!
//! The tree's expand state lives in an external `Signal<HashSet<u64>>`
//! (NOT idea-ui `Collapsible`, whose internal open-state would reset on
//! every poll). The visible-row list is a [`memo`] over `(tree, expanded,
//! selected)`, so expanding a branch, selecting a node, or a new poll all
//! rebuild the rows — and each row carries its own `__expanded`/`__selected`
//! flags, so the chevron and `ListItem` highlight are correct per build
//! (keyed reconciliation keeps it cheap).

use std::collections::HashSet;
use std::rc::Rc;

use idea_ui::{
    typography_kind, Button, Icon, Stack, StackAlign, StackAxis, StackGap, StackPadding, Surface,
    SurfaceColor, Typography,
};
use runtime_core::{
    component, memo, pressable, text, text_input, ui, view, Color, Element, IntoElement, Length,
    Signal, StyleApplication, StyleRules, StyleSheet, Tokenized, VariantSet,
};
use serde_json::{json, Value};

use crate::client::Snapshot;
use crate::{client_action, format};

/// Tail cap on rendered log lines (after filtering).
const LOG_TAIL: usize = 250;
/// Max label width before truncation in a tree row.
const LABEL_MAX: usize = 36;
/// Per-depth indent (px) applied to a tree row's left padding.
const INDENT_PX: f32 = 14.0;
/// Square size (px) of the chevron icon / the leaf placeholder slot. A
/// touch below the body text size reads better against the labels.
const CHEVRON_PX: f32 = 13.0;

// =============================================================================
// Elements — master/detail (collapsible tree + detail pane)
// =============================================================================

#[derive(Default)]
pub struct ElementsPanelProps {
    pub snapshot: Signal<Snapshot>,
    /// Currently-selected element id (drives the detail pane).
    pub selected: Signal<Option<u64>>,
    /// Set of expanded branch ids. Lives outside the row build so it
    /// survives poll-driven rebuilds.
    pub expanded: Signal<HashSet<u64>>,
    /// JSON args buffer shared by the detail pane's method-invoke controls.
    pub invoke_arg: Signal<String>,
}

/// Walk the tree depth-first, emitting one flat row per *visible* node — a
/// node is visible if every ancestor is expanded. Each row is a shallow
/// clone of the node (its `children` array stripped) plus synthetic
/// `__depth` / `__has_children` / `__expanded` fields the row renderer
/// reads. NOTE selection is deliberately *not* folded in here — that would
/// put `selected` in the rows `memo`, rebuilding the entire list (and
/// flickering) on every tap. Selection is a per-row *reactive style*
/// instead (see [`tree_row`]). Only the tree shape + expand state drive
/// the `memo`, so the row list rebuilds only when it genuinely changes.
/// Offset added to an element id to form a synthetic, collision-free row key
/// for the component node sitting above that element. Real element ids are
/// `u32`, so anything `>= 1<<40` is unambiguously a component row; strip it
/// back via [`real_element_id`] to resolve methods/detail.
const COMPONENT_ROW_OFFSET: u64 = 1 << 40;

/// The element id a selected row resolves to — strips the component-row
/// offset so a component node and its root element both map to the same
/// element (for the detail pane + method lookup).
fn real_element_id(selected: u64) -> u64 {
    if selected >= COMPONENT_ROW_OFFSET {
        selected - COMPONENT_ROW_OFFSET
    } else {
        selected
    }
}

/// Flatten the tree into visible rows. When an element is a `#[component]`
/// root (its id is a key in `component_roots`), a synthetic **component node**
/// is emitted directly above it and the element is nested one level deeper —
/// so the tree reads `MethodCounter → View → …` rather than a bare `View`.
fn build_visible(
    nodes: &[Value],
    depth: usize,
    expanded: &HashSet<u64>,
    component_roots: &std::collections::HashMap<u64, String>,
    out: &mut Vec<Value>,
) {
    for n in nodes {
        let id = n["id"].as_u64().unwrap_or(0);
        let children = n["children"].as_array();
        let has_children = children.map(|c| !c.is_empty()).unwrap_or(false);
        let is_expanded = expanded.contains(&id);

        // A component root gets a synthetic parent row carrying the component
        // name; the element itself indents under it.
        let node_depth = if let Some(name) = component_roots.get(&id) {
            out.push(json!({
                "id": id + COMPONENT_ROW_OFFSET,
                "__depth": depth,
                "__is_component": true,
                "__component_name": name,
            }));
            depth + 1
        } else {
            depth
        };

        let mut row = n.clone();
        if let Some(obj) = row.as_object_mut() {
            obj.remove("children");
            obj.insert("__depth".to_string(), json!(node_depth));
            obj.insert("__has_children".to_string(), json!(has_children));
            obj.insert("__expanded".to_string(), json!(is_expanded));
        }
        out.push(row);
        if has_children && is_expanded {
            build_visible(children.unwrap(), node_depth + 1, expanded, component_roots, out);
        }
    }
}

/// The label shown on a tree row: `kind #test_id  "label"`.
fn node_label(row: &Value) -> String {
    let kind = row["kind"].as_str().unwrap_or("?");
    let mut s = kind.to_string();
    if let Some(tid) = row["test_id"].as_str() {
        s.push_str(&format!(" #{tid}"));
    }
    if let Some(label) = row["label"].as_str() {
        if !label.is_empty() {
            s.push_str(&format!("  \"{}\"", format::truncate(label, LABEL_MAX)));
        }
    }
    s
}

/// A fixed-width empty box — the leaf chevron placeholder, so leaf labels
/// line up under their expandable siblings.
fn slot(width: f32) -> Element {
    let style = Rc::new(StyleSheet::new(move |_vs: &VariantSet| StyleRules {
        width: Some(Tokenized::Literal(Length::Px(width))),
        ..Default::default()
    }));
    view(Vec::new()).with_style(style).into_element()
}

/// Row container sheet: depth-proportional left padding (the indent), tight
/// vertical padding, and a themed highlight (`color-surface-alt`) when
/// selected. Built per (depth, selected) and applied via a reactive style
/// closure so flipping selection re-applies the sheet on the *existing*
/// view — no list rebuild, no flicker.
fn row_sheet(depth: usize, selected: bool) -> Rc<StyleSheet> {
    let pad_left = 6.0 + depth as f32 * INDENT_PX;
    Rc::new(StyleSheet::new(move |_vs: &VariantSet| StyleRules {
        background: selected
            .then(|| Tokenized::token("color-surface-alt", Color("#e5e7eb".to_string()))),
        padding_left: Some(Tokenized::Literal(Length::Px(pad_left))),
        padding_top: Some(Tokenized::Literal(Length::Px(3.0))),
        padding_bottom: Some(Tokenized::Literal(Length::Px(3.0))),
        ..Default::default()
    }))
}

/// One tree row — the entire row is a single tap target (a small icon-only
/// `pressable` is an unreliable hit target on macOS, since the vector icon
/// gives the pressable no solid hittable area). Tapping a branch toggles
/// its expansion *and* selects it; tapping a leaf just selects it. The
/// lucide chevron is a visual indicator; leaves get a same-width
/// placeholder so labels line up. Vertically centered via `StackAlign`.
fn tree_row(row: Value, selected: Signal<Option<u64>>, expanded: Signal<HashSet<u64>>) -> Element {
    let id = row["id"].as_u64().unwrap_or(0);
    let depth = row["__depth"].as_u64().unwrap_or(0) as usize;

    // Synthetic component node: a labeled, non-collapsible parent (its root
    // element renders below it). Tapping it selects → the detail pane shows
    // its methods. Distinct lucide `COMPONENT` glyph instead of a chevron.
    if row["__is_component"].as_bool().unwrap_or(false) {
        let name = row["__component_name"].as_str().unwrap_or("Component").to_string();
        let icon = ui! {
            Icon(data = icons_lucide::COMPONENT, size = CHEVRON_PX, color = Some(Color("#7c3aed".to_string())))
        };
        let inner = ui! {
            Stack(axis = StackAxis::Row, align = StackAlign::Center, gap = StackGap::Xs) {
                icon
                text(name).into_element()
            }
        };
        return pressable(vec![inner], move || selected.set(Some(id)))
            .with_style(move || StyleApplication::new(row_sheet(depth, selected.get() == Some(id))))
            .into_element();
    }

    let has_children = row["__has_children"].as_bool().unwrap_or(false);
    let is_expanded = row["__expanded"].as_bool().unwrap_or(false);

    let chevron: Element = if has_children {
        let glyph = if is_expanded {
            icons_lucide::CHEVRON_DOWN
        } else {
            icons_lucide::CHEVRON_RIGHT
        };
        // Explicit color — inside a pressable the ambient text color isn't
        // reliable, which left the bare-inherited icon invisible.
        ui! { Icon(data = glyph, size = CHEVRON_PX, color = Some(Color("#4b5563".to_string()))) }
    } else {
        slot(CHEVRON_PX)
    };

    let label = node_label(&row);
    let inner = ui! {
        Stack(axis = StackAxis::Row, align = StackAlign::Center, gap = StackGap::Xs) {
            chevron
            text(label).into_element()
        }
    };

    pressable(vec![inner], move || {
        selected.set(Some(id));
        if has_children {
            // Toggle membership: `insert` returns false when already present.
            expanded.update(|s| {
                if !s.insert(id) {
                    s.remove(&id);
                }
            });
        }
    })
    // Reactive selection highlight: re-applies on the existing view when
    // `selected` changes, so a tap doesn't rebuild the list.
    .with_style(move || StyleApplication::new(row_sheet(depth, selected.get() == Some(id))))
    .into_element()
}

/// Detail block for the selected element (looked up in the flat `find_all`
/// list, which carries the same fields as tree nodes).
fn element_detail(snap: &Snapshot, id: u64) -> String {
    let Some(e) = snap.elements.iter().find(|e| e["id"].as_u64() == Some(id)) else {
        return format!("element #{id} is no longer registered.");
    };
    let mut out = format!("id: {id}\nkind: {}\n", e["kind"].as_str().unwrap_or("?"));
    out.push_str(&format!(
        "test_id: {}\n",
        e["test_id"].as_str().unwrap_or("—")
    ));
    let label = e["label"].as_str().unwrap_or("");
    out.push_str(&format!(
        "label: {}\n",
        if label.is_empty() { "—" } else { label }
    ));
    out
}

fn method_signature(method: &Value) -> String {
    let name = method["name"].as_str().unwrap_or("?");
    let args: Vec<String> = method["args"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|arg| {
                    format!(
                        "{}: {}",
                        arg["name"].as_str().unwrap_or("?"),
                        arg["type"].as_str().unwrap_or("?")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    format!(".{}({})", name, args.join(", "))
}

/// `true` if the selected element is a `#[component]` root with methods.
/// Drives the reactive `if` that shows the invoke section.
fn linked_component_present(snap: &Snapshot, selected: Option<u64>) -> bool {
    selected
        .map(|sel| {
            let eid = real_element_id(sel);
            snap.components
                .iter()
                .any(|c| c["element_id"].as_u64() == Some(eid))
        })
        .unwrap_or(false)
}

/// The invoke section for the selected element's linked component: one row
/// per method (an Invoke button + signature), plus a shared JSON args field
/// when any method takes arguments. Rebuilt by the enclosing reactive `if`
/// whenever the selection (or snapshot) changes.
fn method_invoke_section(
    snapshot: Signal<Snapshot>,
    selected: Signal<Option<u64>>,
    arg_json: Signal<String>,
) -> Element {
    let snap = snapshot.get();
    let comp = selected.get().and_then(|sel| {
        let eid = real_element_id(sel);
        snap.components
            .iter()
            .find(|c| c["element_id"].as_u64() == Some(eid))
            .cloned()
    });

    let mut kids: Vec<Element> = Vec::new();
    if let Some(c) = comp {
        let id = c["instance_id"].as_u64().unwrap_or(0);
        let name = c["name"].as_str().unwrap_or("?").to_string();
        kids.push(ui! { Typography(content = format!("Methods · {name}"), kind = typography_kind::Overline) });

        let methods = c["methods"].as_array().cloned().unwrap_or_default();
        if methods.is_empty() {
            kids.push(text("(this component exposes no methods)").into_element());
        }
        let mut any_args = false;
        for m in &methods {
            let mname = m["name"].as_str().unwrap_or("?").to_string();
            let has_args = m["args"].as_array().map(|a| !a.is_empty()).unwrap_or(false);
            any_args |= has_args;
            let sig = method_signature(m);
            let on_invoke: Rc<dyn Fn()> = Rc::new(move || {
                let args: Value = if has_args {
                    serde_json::from_str(arg_json.get().trim()).unwrap_or_else(|_| json!({}))
                } else {
                    json!({})
                };
                client_action(
                    "invoke_method",
                    json!({ "instance_id": id, "method": mname, "args": args }),
                );
            });
            kids.push(ui! {
                Stack(axis = StackAxis::Row, align = StackAlign::Center, gap = StackGap::Sm) {
                    Button(label = "Invoke".to_string(), on_click = on_invoke)
                    text(sig).into_element()
                }
            });
        }
        if any_args {
            kids.push(
                text_input(arg_json, move |v| arg_json.set(v))
                    .placeholder("args JSON, e.g. {\"n\": 5}".to_string())
                    .into_element(),
            );
        }
    }

    ui! { Stack(gap = StackGap::Xs) { kids } }
}

/// Master/detail element inspector. Left: a collapsible, selectable tree.
/// Right: the selected element's details and, when it's a `#[component]`
/// root, its invokable methods.
#[component]
pub fn ElementsPanel(props: ElementsPanelProps) -> Element {
    let ElementsPanelProps { snapshot, selected, expanded, invoke_arg } = props;

    // Visible rows — recompute on tree / expand change.
    let rows: Signal<Vec<Value>> = memo(move || {
        let snap = snapshot.get();
        let exp = expanded.get();
        // element_id → component name, so a component root gets a synthetic
        // parent node in the tree.
        let mut roots: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
        for c in &snap.components {
            if let (Some(eid), Some(name)) = (c["element_id"].as_u64(), c["name"].as_str()) {
                roots.insert(eid, name.to_string());
            }
        }
        let mut out = Vec::new();
        build_visible(&snap.tree, 0, &exp, &roots, &mut out);
        out
    });

    // Detail body (expr reads `.get()` → macro reactive-wraps it).
    let detail = text(match selected.get() {
        Some(id) => element_detail(&snapshot.get(), real_element_id(id)),
        None => "Select an element on the left to inspect it.".to_string(),
    })
    .into_element();

    // Master/detail: a recessed (gray) tree pane beside a raised (white)
    // detail panel. `align = Stretch` makes the two panes equal height.
    ui! {
        Stack(axis = StackAxis::Row, align = StackAlign::Stretch, gap = StackGap::Xs) {
            Surface(background = SurfaceColor::Background, grow = 2.0, padding = StackPadding::Sm) {
                for row in rows, key = row["id"].as_u64().unwrap_or(0) {
                    tree_row(row, selected, expanded)
                }
            }
            Surface(background = SurfaceColor::Surface, grow = 3.0, padding = StackPadding::Md) {
                Stack(gap = StackGap::Sm) {
                    Typography(content = "Element", kind = typography_kind::H3)
                    detail
                    // Methods appear only when the selected element is a
                    // `#[component]` root the framework linked to an instance.
                    if linked_component_present(&snapshot.get(), selected.get()) {
                        method_invoke_section(snapshot, selected, invoke_arg)
                    }
                }
            }
        }
    }
}

// =============================================================================
// Logs — filterable stream
// =============================================================================

#[derive(Default)]
pub struct LogsPanelProps {
    pub snapshot: Signal<Snapshot>,
    pub filter: Signal<String>,
}

fn log_matches(e: &Value, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let src = e["source"].as_str().unwrap_or("");
    let txt = e["text"].as_str().unwrap_or("");
    src.to_lowercase().contains(needle) || txt.to_lowercase().contains(needle)
}

fn log_count_line(snap: &Snapshot, needle: &str) -> String {
    let shown = snap.logs.iter().filter(|e| log_matches(e, needle)).count();
    format!("{}/{} shown", shown, snap.logs.len())
}

fn log_body(snap: &Snapshot, needle: &str) -> String {
    let matched: Vec<String> = snap
        .logs
        .iter()
        .filter(|e| log_matches(e, needle))
        .map(|e| {
            format!(
                "[{}] {}",
                e["source"].as_str().unwrap_or("?"),
                e["text"].as_str().unwrap_or(""),
            )
        })
        .collect();
    if matched.is_empty() {
        return if snap.logs.is_empty() {
            "(no logs captured)".to_string()
        } else {
            "(no logs match filter)".to_string()
        };
    }
    let start = matched.len().saturating_sub(LOG_TAIL);
    matched[start..].join("\n")
}

/// The captured log stream with a live substring filter (matches source or
/// text, case-insensitive) and a Clear button.
#[component]
pub fn LogsPanel(props: LogsPanelProps) -> Element {
    let LogsPanelProps { snapshot, filter } = props;
    let filter_input = text_input(filter, move |v| filter.set(v))
        .placeholder("filter (substring of source or text)…".to_string())
        .into_element();
    let clear: Rc<dyn Fn()> = Rc::new(|| client_action("clear_logs", json!({})));
    let clear_btn = ui! { Button(label = "Clear".to_string(), on_click = clear) };

    // Reactive (exprs read `.get()` → the macro reactive-wraps them).
    let count = text(log_count_line(&snapshot.get(), &filter.get().to_lowercase())).into_element();
    let body = text(log_body(&snapshot.get(), &filter.get().to_lowercase())).into_element();

    ui! {
        Stack(gap = StackGap::Sm) {
            Stack(axis = StackAxis::Row, gap = StackGap::Sm) {
                filter_input
                clear_btn
            }
            count
            body
        }
    }
}

// =============================================================================
// Stats — read-only runtime internals
// =============================================================================

#[derive(Default)]
pub struct StatsPanelProps {
    pub snapshot: Signal<Snapshot>,
}

/// A compact one-line-per-navigator summary (the dedicated navigator UI was
/// dropped from the tab bar; this keeps the introspection visible).
fn navigators_text(snap: &Snapshot) -> String {
    if snap.navigators.is_empty() {
        return "(none)".to_string();
    }
    let mut out = String::new();
    for n in &snap.navigators {
        let current = if n["is_current"].as_bool().unwrap_or(false) {
            "● "
        } else {
            "  "
        };
        out.push_str(&format!(
            "{current}{}  route={}  depth={}  back={}\n",
            format::short_kind(n["type_name"].as_str().unwrap_or("?")),
            n["active_route"].as_str().unwrap_or("?"),
            n["depth"].as_u64().unwrap_or(0),
            n["can_go_back"].as_bool().unwrap_or(false),
        ));
    }
    out
}

/// Read-only diagnostics: navigators, arena stats, perf phase counters,
/// watched signals, and the flat `find_all` element list.
#[component]
pub fn StatsPanel(props: StatsPanelProps) -> Element {
    let StatsPanelProps { snapshot } = props;
    // `text(<expr with .get()>)` → reactive-wrapped, refreshes each poll.
    let navigators = text(navigators_text(&snapshot.get())).into_element();
    let perf = text(format::perf(&snapshot.get())).into_element();
    let signals = text(format::signals(&snapshot.get())).into_element();
    let raw = text(format::raw_elements(&snapshot.get())).into_element();
    ui! {
        Stack(gap = StackGap::Md, padding = StackPadding::Sm) {
            Typography(content = "NAVIGATORS", kind = typography_kind::Overline)
            navigators
            Typography(content = "ARENA / PERF", kind = typography_kind::Overline)
            perf
            Typography(content = "WATCHED SIGNALS", kind = typography_kind::Overline)
            signals
            Typography(content = "RAW ELEMENTS (find_all)", kind = typography_kind::Overline)
            raw
        }
    }
}
