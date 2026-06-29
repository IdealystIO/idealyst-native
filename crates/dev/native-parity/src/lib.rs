//! Cross-platform render-parity: model + diff over the `introspect_native`
//! robot verb.
//!
//! The framework's promise is that one author tree renders the same on every
//! backend. `introspect_native` lets us *measure* that — it returns each
//! primitive's **platform-resolved** geometry + visual props, read from the
//! live native object (a `CALayer`, the DOM's `getComputedStyle`), not the
//! styles the author asked for. Capture that from two running apps and diff the
//! canonical prop maps key-by-key to find where the platforms actually diverge.
//!
//! This crate is the **shared, I/O-free** half:
//! - the wire model ([`NativeNode`] / [`PropValue`]),
//! - [`element_paths`] — turn a `get_snapshot` tree into the `(path, id)` list a
//!   caller introspects element-by-element, keyed by a stable path so the same
//!   authored element lines up across platforms,
//! - [`diff`] — compare two [`Capture`]s with per-type [`Tolerance`].
//!
//! The actual bridge calls (sync in `robot-test`, async in the MCP server) live
//! in the callers; only the model + path-walk + diff are shared, so the
//! comparison logic is written — and unit-tested — once.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::Value;

/// A platform-resolved property value, deserialized from the `introspect_native`
/// wire form (`{ "type": "...", "value": ... }`). Mirrors
/// `runtime_core::introspect::NativeValue` — the wire contract is the shared
/// surface, so this crate stays free of the framework dep.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PropValue {
    Color([f32; 4]),
    Length(f32),
    Number(f32),
    Text(String),
    Flag(bool),
}

/// One native node as reported by a backend. `class` (the platform class/tag)
/// is intentionally **not** compared across platforms — `NSTextField` vs
/// `input` is expected to differ; the *resolved props* are what must match.
#[derive(Debug, Clone, Deserialize)]
pub struct NativeNode {
    pub class: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub props: BTreeMap<String, PropValue>,
    #[serde(default)]
    pub children: Vec<NativeNode>,
}

/// A captured tree: stable element path → that element's native node. A
/// `BTreeMap` so iteration (and any report) is deterministic.
pub type Capture = BTreeMap<String, NativeNode>;

/// Extract `(stable_path, element_id)` for every element in a `get_snapshot`
/// payload, in depth-first order. The path is `test_id`-anchored where present
/// (durable + identical across platforms), else `kind[index]` accumulated down
/// the tree. A caller introspects each id and inserts into a [`Capture`] under
/// its path.
///
/// Accepts either the array-of-roots or single-root shape `get_snapshot`
/// returns, so it isn't brittle to that detail.
pub fn element_paths(snapshot: &Value) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    match snapshot {
        Value::Array(roots) => {
            for (i, r) in roots.iter().enumerate() {
                collect_paths(r, &i.to_string(), &mut out);
            }
        }
        node => collect_paths(node, "0", &mut out),
    }
    out
}

fn collect_paths(node: &Value, path: &str, out: &mut Vec<(String, u64)>) {
    if let Some(id) = node.get("id").and_then(Value::as_u64) {
        out.push((path.to_string(), id));
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        let mut kind_counts: BTreeMap<String, usize> = BTreeMap::new();
        for child in children {
            let kind = child.get("kind").and_then(Value::as_str).unwrap_or("?");
            let idx = kind_counts.entry(kind.to_string()).or_insert(0);
            let seg = match child.get("test_id").and_then(Value::as_str) {
                Some(tid) => format!("#{tid}"),
                None => format!("{kind}[{idx}]"),
            };
            *idx += 1;
            collect_paths(child, &format!("{path}/{seg}"), out);
        }
    }
}

// ===========================================================================
// Structural alignment
//
// Positional paths (`element_paths`) collapse the moment two platforms produce
// even slightly different tree shapes — an extra wrapper, a reordered sibling,
// a different root count — because the index of every later node shifts. That
// makes the diff report misattribute unrelated elements to each other.
//
// `align` instead matches the two `get_snapshot` trees by a cross-platform
// **signature** (the framework `kind` + `test_id`/`label`, which are identical
// across backends — only the *native class* differs), greedily and in order.
// This tolerates inserted/removed/reordered siblings: an extra node on one side
// is reported as structurally unmatched rather than throwing off everything
// after it. Aligned pairs get a stable signature-based path; the caller
// introspects each side's id at that path and diffs only genuinely-corresponding
// elements.
//
// Limitation: greedy sibling matching does not see through *wrapper-depth*
// differences (A nesting `View > View > Text` vs B `View > Text`) — the extra
// wrapper, and everything under it, reports as unmatched. That's a genuine
// structural divergence worth surfacing, not silently bridging.
// ===========================================================================

/// A framework element from `get_snapshot`. `kind`/`test_id`/`label` are stable
/// across platforms (the native *class* is not), so alignment keys on these.
#[derive(Debug, Clone, Deserialize)]
pub struct SnapNode {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub test_id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub children: Vec<SnapNode>,
}

/// Parse a `get_snapshot` payload (array-of-roots or single root) into the
/// framework tree alignment operates on.
pub fn parse_snapshot(snapshot: &Value) -> Vec<SnapNode> {
    match snapshot {
        Value::Array(roots) => roots
            .iter()
            .filter_map(|r| serde_json::from_value(r.clone()).ok())
            .collect(),
        node => serde_json::from_value(node.clone()).ok().into_iter().collect(),
    }
}

/// Find the subtree rooted at the element carrying `test_id` (depth-first),
/// or `None` if no element has it. Use to **scope** a comparison to a content
/// anchor — aligning from this subtree excludes the navigator chrome, which is
/// built per-platform with deliberately different native structure and so
/// can't (and shouldn't) be diffed element-by-element.
pub fn subtree_by_test_id<'a>(nodes: &'a [SnapNode], test_id: &str) -> Option<&'a SnapNode> {
    for n in nodes {
        if n.test_id.as_deref() == Some(test_id) {
            return Some(n);
        }
        if let Some(found) = subtree_by_test_id(&n.children, test_id) {
            return Some(found);
        }
    }
    None
}

/// The cross-platform match key for a node: its `test_id` when present (the
/// durable, refactor-proof anchor), else `kind|label` (text/button label), else
/// bare `kind` (containers — matched among same-kind siblings by order).
fn signature(n: &SnapNode) -> String {
    if let Some(t) = &n.test_id {
        return format!("#{t}");
    }
    match &n.label {
        Some(l) if !l.is_empty() => format!("{}|{}", n.kind, l),
        _ => n.kind.clone(),
    }
}

/// One element matched on both sides: the same logical node, with a stable
/// signature-based path (not a fragile positional index).
#[derive(Debug, Clone, PartialEq)]
pub struct AlignedPair {
    pub path: String,
    pub id_a: u64,
    pub id_b: u64,
}

/// A node present on only one side — a structural divergence between the two
/// trees (an element one platform renders and the other doesn't).
#[derive(Debug, Clone, PartialEq)]
pub struct Unmatched {
    pub path: String,
    pub id: u64,
    pub kind: String,
    /// `true` = only in tree A, `false` = only in tree B.
    pub in_a: bool,
}

/// Result of aligning two framework trees.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Alignment {
    pub pairs: Vec<AlignedPair>,
    pub unmatched: Vec<Unmatched>,
}

/// Align two `get_snapshot` trees (A, B) by signature, returning the
/// corresponding element pairs plus the structurally-unmatched nodes on each
/// side. See the module-level note for the matching strategy + its limitation.
pub fn align(a: &[SnapNode], b: &[SnapNode]) -> Alignment {
    let mut out = Alignment::default();
    align_lists(a, b, "", &mut out);
    out
}

fn align_lists(a: &[SnapNode], b: &[SnapNode], parent: &str, out: &mut Alignment) {
    let mut b_used = vec![false; b.len()];
    // Per-signature occurrence counter under this parent, so two same-signature
    // siblings get distinct, stable paths (`View#0`, `View#1`).
    let mut occ: BTreeMap<String, usize> = BTreeMap::new();
    let seg_path = |parent: &str, seg: &str| {
        if parent.is_empty() { seg.to_string() } else { format!("{parent}/{seg}") }
    };

    for na in a {
        let sig = signature(na);
        let n = occ.entry(sig.clone()).or_insert(0);
        let seg = format!("{sig}#{n}");
        *n += 1;
        let path = seg_path(parent, &seg);

        // Greedy: the first not-yet-used B sibling with the same signature, in
        // order — so insertions/removals/reorders don't cascade.
        let matched = b
            .iter()
            .enumerate()
            .find(|(j, nb)| !b_used[*j] && signature(nb) == sig)
            .map(|(j, _)| j);

        match matched {
            Some(j) => {
                b_used[j] = true;
                out.pairs.push(AlignedPair { path: path.clone(), id_a: na.id, id_b: b[j].id });
                align_lists(&na.children, &b[j].children, &path, out);
            }
            None => {
                // Report the unmatched subtree at its root (not every descendant
                // — one "missing here" line, not a flood).
                out.unmatched.push(Unmatched {
                    path,
                    id: na.id,
                    kind: na.kind.clone(),
                    in_a: true,
                });
            }
        }
    }

    for (j, nb) in b.iter().enumerate() {
        if b_used[j] {
            continue;
        }
        let sig = signature(nb);
        let n = occ.entry(sig.clone()).or_insert(0);
        let seg = format!("{sig}#{n}");
        *n += 1;
        out.unmatched.push(Unmatched {
            path: seg_path(parent, &seg),
            id: nb.id,
            kind: nb.kind.clone(),
            in_a: false,
        });
    }
}

/// Deserialize one element's `introspect_native` payload. `null` (the backend
/// has no native data for it yet) yields `Ok(None)`.
pub fn parse_native(value: Value) -> Result<Option<NativeNode>, serde_json::Error> {
    if value.is_null() {
        return Ok(None);
    }
    serde_json::from_value(value).map(Some)
}

/// A single parity difference at one element + property.
#[derive(Debug, Clone, PartialEq)]
pub struct Mismatch {
    /// The element path both captures aligned on.
    pub path: String,
    /// The canonical prop key (e.g. `background_color`), or `<element>` for a
    /// whole-element presence difference.
    pub key: String,
    pub kind: MismatchKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MismatchKind {
    /// The element exists in one capture but not the other.
    ElementMissing { in_a: bool },
    /// The prop is present on one side only.
    PropMissing { in_a: bool },
    /// Both have the prop but the values differ beyond tolerance.
    ValueDiffers { a: PropValue, b: PropValue },
}

/// Tolerances for the numeric comparisons. Colors/lengths/numbers compare
/// within an epsilon (sub-pixel rounding + color-space round-trips make exact
/// equality the wrong test); text/flags compare exactly.
#[derive(Debug, Clone, Copy)]
pub struct Tolerance {
    pub color: f32,
    pub length: f32,
    pub number: f32,
}

impl Default for Tolerance {
    fn default() -> Self {
        Self { color: 0.02, length: 0.5, number: 0.5 }
    }
}

/// Font/text props. Web's `getComputedStyle` reports these (inherited via the
/// CSS cascade) on *every* element; macOS only on actual text widgets. So
/// comparing them on a container is meaningless — they're only compared when
/// both sides are text-bearing.
const TEXT_PROPS: &[&str] =
    &["font_family", "font_size", "font_weight", "text_color", "text"];

/// Options for [`diff_with`].
#[derive(Debug, Clone, Copy)]
pub struct DiffOptions {
    pub tol: Tolerance,
    /// Apply cross-platform **representation** normalization (recommended for
    /// web↔native parity, where the two backends encode the same visual state
    /// differently):
    /// - font/text props are compared only on text-bearing elements (web
    ///   reports inherited fonts everywhere; macOS doesn't),
    /// - a fully-transparent color counts as equivalent to an absent one (web
    ///   paints `rgba(0,0,0,0)`; a native view simply has no layer color),
    /// - `font_family` names are matched by family *class* (`-apple-system` ≡
    ///   `.AppleSystemUIFont` = "system"; `ui-monospace` ≡ `Menlo` = "mono").
    ///
    /// Off → a raw, key-by-key compare (every representation difference shows).
    pub normalize: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self { tol: Tolerance::default(), normalize: true }
    }
}

/// Diff two captures (`a`, `b`) with default options (cross-platform
/// normalization on). See [`diff_with`].
pub fn diff(a: &Capture, b: &Capture, tol: Tolerance) -> Vec<Mismatch> {
    diff_with(a, b, DiffOptions { tol, ..Default::default() })
}

/// Diff two captures, reporting every element/prop divergence that survives the
/// chosen normalization. An empty result means the two backends resolved
/// equivalent canonical render state for every aligned element — full parity.
pub fn diff_with(a: &Capture, b: &Capture, opts: DiffOptions) -> Vec<Mismatch> {
    let mut out = Vec::new();

    for path in a.keys() {
        if !b.contains_key(path) {
            out.push(Mismatch { path: path.clone(), key: "<element>".into(),
                kind: MismatchKind::ElementMissing { in_a: true } });
        }
    }
    for path in b.keys() {
        if !a.contains_key(path) {
            out.push(Mismatch { path: path.clone(), key: "<element>".into(),
                kind: MismatchKind::ElementMissing { in_a: false } });
        }
    }

    for (path, na) in a {
        let Some(nb) = b.get(path) else { continue };
        let text_ok = !opts.normalize
            || (na.props.contains_key("text") && nb.props.contains_key("text"));
        // Union of prop keys, deterministic order.
        let keys: BTreeSet<&String> = na.props.keys().chain(nb.props.keys()).collect();
        for key in keys {
            if opts.normalize && TEXT_PROPS.contains(&key.as_str()) && !text_ok {
                continue; // font/text prop on a non-text element — skip the noise
            }
            match (na.props.get(key), nb.props.get(key)) {
                (Some(va), Some(vb)) => {
                    if !values_equivalent(key, va, vb, &opts) {
                        out.push(Mismatch { path: path.clone(), key: key.clone(),
                            kind: MismatchKind::ValueDiffers { a: va.clone(), b: vb.clone() } });
                    }
                }
                (present_a, present_b) => {
                    let present = present_a.or(present_b);
                    // Transparent on one side ≡ absent on the other.
                    if opts.normalize && key == "background_color"
                        && present.map(is_transparent).unwrap_or(false)
                    {
                        continue;
                    }
                    out.push(Mismatch { path: path.clone(), key: key.clone(),
                        kind: MismatchKind::PropMissing { in_a: present_a.is_some() } });
                }
            }
        }
    }
    out
}

fn is_transparent(v: &PropValue) -> bool {
    matches!(v, PropValue::Color([_, _, _, a]) if *a <= 0.01)
}

/// Family *class* for a `font-family` value, so the same physical font under
/// different platform names compares equal. `-apple-system` / `.AppleSystemUIFont`
/// / `system-ui` → "system"; `ui-monospace` / `Menlo` / `SFMono` → "mono".
fn canonical_font(s: &str) -> String {
    let s = s.trim().trim_start_matches('.').to_lowercase();
    const SYSTEM: &[&str] =
        &["applesystemuifont", "apple-system", "system-ui", "sfns", "helvetica neue", "sfpro"];
    const MONO: &[&str] = &["monospace", "sfmono", "menlo", "monaco", "courier", "consolas"];
    if SYSTEM.iter().any(|m| s.contains(m)) {
        return "system".into();
    }
    if MONO.iter().any(|m| s.contains(m)) {
        return "mono".into();
    }
    s
}

fn values_equivalent(key: &str, a: &PropValue, b: &PropValue, opts: &DiffOptions) -> bool {
    if opts.normalize && key == "font_family" {
        if let (PropValue::Text(x), PropValue::Text(y)) = (a, b) {
            return canonical_font(x) == canonical_font(y);
        }
    }
    values_match(a, b, opts.tol)
}

fn values_match(a: &PropValue, b: &PropValue, tol: Tolerance) -> bool {
    match (a, b) {
        (PropValue::Color(x), PropValue::Color(y)) => {
            x.iter().zip(y).all(|(p, q)| (p - q).abs() <= tol.color)
        }
        (PropValue::Length(x), PropValue::Length(y)) => (x - y).abs() <= tol.length,
        (PropValue::Number(x), PropValue::Number(y)) => (x - y).abs() <= tol.number,
        (PropValue::Text(x), PropValue::Text(y)) => x == y,
        (PropValue::Flag(x), PropValue::Flag(y)) => x == y,
        _ => false,
    }
}

/// Render a diff as a human-readable report (one line per mismatch). Empty
/// string when there are no mismatches. Labels A/B with the caller-provided
/// platform names.
pub fn report(mismatches: &[Mismatch], a_label: &str, b_label: &str) -> String {
    let mut lines = Vec::new();
    for m in mismatches {
        let detail = match &m.kind {
            MismatchKind::ElementMissing { in_a: true } => format!("only in {a_label}"),
            MismatchKind::ElementMissing { in_a: false } => format!("only in {b_label}"),
            MismatchKind::PropMissing { in_a: true } => format!("prop only in {a_label}"),
            MismatchKind::PropMissing { in_a: false } => format!("prop only in {b_label}"),
            MismatchKind::ValueDiffers { a, b } => format!("{a_label}={a:?} {b_label}={b:?}"),
        };
        lines.push(format!("{}  {}  {}", m.path, m.key, detail));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(props: &[(&str, PropValue)]) -> NativeNode {
        NativeNode {
            class: "x".into(),
            role: None,
            props: props.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            children: Vec::new(),
        }
    }

    fn cap(path: &str, n: NativeNode) -> Capture {
        let mut c = Capture::new();
        c.insert(path.into(), n);
        c
    }

    #[test]
    fn normalize_skips_font_props_on_non_text_elements() {
        // A container: web reports an inherited font, macOS reports none.
        // Without `text` on both sides, font props must not be compared.
        let web = cap("0", node(&[("font_family", PropValue::Text("-apple-system".into()))]));
        let mac = cap("0", node(&[]));
        assert!(diff(&web, &mac, Tolerance::default()).is_empty(), "font on a container = noise");
        // …but with normalize OFF it shows.
        let raw = diff_with(&web, &mac, DiffOptions { normalize: false, ..Default::default() });
        assert_eq!(raw.len(), 1);
    }

    #[test]
    fn normalize_treats_transparent_bg_as_absent() {
        let web = cap("0", node(&[("background_color", PropValue::Color([0.0, 0.0, 0.0, 0.0]))]));
        let mac = cap("0", node(&[])); // native view has no layer color
        assert!(diff(&web, &mac, Tolerance::default()).is_empty());
        // An OPAQUE color absent on the other side IS a real divergence.
        let web2 = cap("0", node(&[("background_color", PropValue::Color([1.0, 0.0, 0.0, 1.0]))]));
        assert_eq!(diff(&web2, &mac, Tolerance::default()).len(), 1);
    }

    #[test]
    fn normalize_aliases_system_and_mono_font_names() {
        // Same physical font, different platform names → parity.
        let web = cap("0", node(&[
            ("text", PropValue::Text("Hi".into())),
            ("font_family", PropValue::Text("-apple-system".into())),
        ]));
        let mac = cap("0", node(&[
            ("text", PropValue::Text("Hi".into())),
            ("font_family", PropValue::Text(".AppleSystemUIFont".into())),
        ]));
        assert!(diff(&web, &mac, Tolerance::default()).is_empty(), "system aliases match");

        // But monospace-vs-system is a REAL divergence (the bug parity should catch).
        let mac_mono = cap("0", node(&[
            ("text", PropValue::Text("Hi".into())),
            ("font_family", PropValue::Text(".AppleSystemUIFont".into())),
        ]));
        let web_mono = cap("0", node(&[
            ("text", PropValue::Text("Hi".into())),
            ("font_family", PropValue::Text("ui-monospace".into())),
        ]));
        let d = diff(&web_mono, &mac_mono, Tolerance::default());
        assert_eq!(d.len(), 1, "mono vs system must surface");
        assert_eq!(d[0].key, "font_family");
    }

    #[test]
    fn identical_captures_have_no_mismatches() {
        let mut a = Capture::new();
        a.insert("0/#title".into(), node(&[
            ("background_color", PropValue::Color([1.0, 0.0, 0.0, 1.0])),
            ("font_size", PropValue::Length(14.0)),
        ]));
        let b = a.clone();
        assert!(diff(&a, &b, Tolerance::default()).is_empty());
    }

    #[test]
    fn within_tolerance_is_parity() {
        let mut a = Capture::new();
        a.insert("0".into(), node(&[("background_color", PropValue::Color([0.50, 0.25, 0.0, 1.0]))]));
        let mut b = Capture::new();
        b.insert("0".into(), node(&[("background_color", PropValue::Color([0.51, 0.24, 0.0, 1.0]))]));
        assert!(diff(&a, &b, Tolerance::default()).is_empty());
    }

    #[test]
    fn value_beyond_tolerance_is_reported() {
        // Text-bearing both sides (so the font prop is in scope), font_size out
        // of tolerance.
        let mut a = Capture::new();
        a.insert("0".into(), node(&[
            ("text", PropValue::Text("Hi".into())),
            ("font_size", PropValue::Length(14.0)),
        ]));
        let mut b = Capture::new();
        b.insert("0".into(), node(&[
            ("text", PropValue::Text("Hi".into())),
            ("font_size", PropValue::Length(18.0)),
        ]));
        let d = diff(&a, &b, Tolerance::default());
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].key, "font_size");
        assert!(matches!(d[0].kind, MismatchKind::ValueDiffers { .. }));
    }

    #[test]
    fn missing_prop_and_missing_element_are_reported() {
        let mut a = Capture::new();
        a.insert("0".into(), node(&[("opacity", PropValue::Number(1.0))]));
        a.insert("0/#only_web".into(), node(&[]));
        let mut b = Capture::new();
        b.insert("0".into(), node(&[]));
        let d = diff(&a, &b, Tolerance::default());
        assert!(d.iter().any(|m| m.key == "opacity"
            && matches!(m.kind, MismatchKind::PropMissing { in_a: true })));
        assert!(d.iter().any(|m| m.path == "0/#only_web"
            && matches!(m.kind, MismatchKind::ElementMissing { in_a: true })));
    }

    #[test]
    fn variant_or_text_change_is_a_divergence() {
        let mut a = Capture::new();
        a.insert("0".into(), node(&[("text", PropValue::Text("Hi".into()))]));
        let mut b = Capture::new();
        b.insert("0".into(), node(&[("text", PropValue::Text("Hola".into()))]));
        let d = diff(&a, &b, Tolerance::default());
        assert_eq!(d.len(), 1);
        assert!(matches!(d[0].kind, MismatchKind::ValueDiffers { .. }));
    }

    #[test]
    fn native_node_deserializes_from_bridge_json() {
        let wire = serde_json::json!({
            "class": "NSView",
            "role": "view",
            "frame": { "x": 0.0, "y": 0.0, "width": 10.0, "height": 10.0 },
            "props": {
                "background_color": { "type": "color", "value": [1.0, 0.0, 0.0, 1.0] },
                "corner_radius": { "type": "length", "value": 8.0 },
                "hidden": { "type": "flag", "value": false }
            }
        });
        let n = parse_native(wire).unwrap().unwrap();
        assert_eq!(n.class, "NSView");
        assert_eq!(n.props.get("corner_radius"), Some(&PropValue::Length(8.0)));
    }

    #[test]
    fn element_paths_walks_snapshot_with_stable_keys() {
        // Array-of-roots, mixed test_id + kind[index].
        let snap = serde_json::json!([
            { "id": 1, "kind": "View", "children": [
                { "id": 2, "kind": "Text", "test_id": "title", "children": [] },
                { "id": 3, "kind": "Button", "children": [] },
                { "id": 4, "kind": "Button", "children": [] }
            ]}
        ]);
        let paths = element_paths(&snap);
        assert_eq!(paths[0], ("0".to_string(), 1));
        assert!(paths.contains(&("0/#title".to_string(), 2)));
        assert!(paths.contains(&("0/Button[0]".to_string(), 3)));
        assert!(paths.contains(&("0/Button[1]".to_string(), 4)));
    }

    // --- structural alignment ----------------------------------------------

    fn snap(json: serde_json::Value) -> Vec<SnapNode> {
        parse_snapshot(&json)
    }

    fn matched(al: &Alignment, id_a: u64) -> Option<u64> {
        al.pairs.iter().find(|p| p.id_a == id_a).map(|p| p.id_b)
    }

    #[test]
    fn identical_trees_align_completely() {
        let t = serde_json::json!([
            { "id": 1, "kind": "View", "children": [
                { "id": 2, "kind": "Text", "label": "Hi", "children": [] },
                { "id": 3, "kind": "Button", "label": "Go", "children": [] }
            ]}
        ]);
        // Different ids on side B (different platform), same structure/labels.
        let b = serde_json::json!([
            { "id": 11, "kind": "View", "children": [
                { "id": 12, "kind": "Text", "label": "Hi", "children": [] },
                { "id": 13, "kind": "Button", "label": "Go", "children": [] }
            ]}
        ]);
        let al = align(&snap(t), &snap(b));
        assert_eq!(al.unmatched, vec![], "no structural divergence");
        assert_eq!(matched(&al, 1), Some(11));
        assert_eq!(matched(&al, 2), Some(12));
        assert_eq!(matched(&al, 3), Some(13));
    }

    #[test]
    fn inserted_sibling_does_not_cascade() {
        // A has an extra Divider between the two Texts. Positional alignment
        // would mis-pair Text "B" with the Divider and everything after; the
        // signature match keeps "A"/"B" aligned and flags only the Divider.
        let a = snap(serde_json::json!([{ "id": 1, "kind": "View", "children": [
            { "id": 2, "kind": "Text", "label": "A", "children": [] },
            { "id": 9, "kind": "Divider", "children": [] },
            { "id": 3, "kind": "Text", "label": "B", "children": [] }
        ]}]));
        let b = snap(serde_json::json!([{ "id": 1, "kind": "View", "children": [
            { "id": 2, "kind": "Text", "label": "A", "children": [] },
            { "id": 3, "kind": "Text", "label": "B", "children": [] }
        ]}]));
        let al = align(&a, &b);
        assert_eq!(matched(&al, 2), Some(2), "Text A still aligns");
        assert_eq!(matched(&al, 3), Some(3), "Text B still aligns (not shifted onto the divider)");
        assert_eq!(al.unmatched.len(), 1);
        assert!(al.unmatched[0].in_a && al.unmatched[0].kind == "Divider");
    }

    #[test]
    fn reordered_labeled_siblings_match_by_label_not_position() {
        let a = snap(serde_json::json!([{ "id": 1, "kind": "View", "children": [
            { "id": 2, "kind": "Button", "label": "Save", "children": [] },
            { "id": 3, "kind": "Button", "label": "Cancel", "children": [] }
        ]}]));
        let b = snap(serde_json::json!([{ "id": 1, "kind": "View", "children": [
            { "id": 30, "kind": "Button", "label": "Cancel", "children": [] },
            { "id": 20, "kind": "Button", "label": "Save", "children": [] }
        ]}]));
        let al = align(&a, &b);
        assert_eq!(matched(&al, 2), Some(20), "Save→Save by label, despite reorder");
        assert_eq!(matched(&al, 3), Some(30), "Cancel→Cancel by label");
        assert!(al.unmatched.is_empty());
    }

    #[test]
    fn test_id_anchors_across_differing_labels() {
        // Same element, but the visible label differs (e.g. localized) — the
        // test_id still anchors the match.
        let a = snap(serde_json::json!([
            { "id": 2, "kind": "Text", "test_id": "greeting", "label": "Hello", "children": [] }
        ]));
        let b = snap(serde_json::json!([
            { "id": 9, "kind": "Text", "test_id": "greeting", "label": "Hola", "children": [] }
        ]));
        let al = align(&a, &b);
        assert_eq!(matched(&al, 2), Some(9));
        assert!(al.unmatched.is_empty());
    }

    #[test]
    fn subtree_by_test_id_finds_and_scopes() {
        // Chrome (sidebar/header) wraps a content anchor; scoping to the anchor
        // drops the chrome from alignment.
        let tree = snap(serde_json::json!([
            { "id": 1, "kind": "Navigator", "children": [
                { "id": 2, "kind": "ScrollView", "children": [
                    { "id": 3, "kind": "Text", "label": "Sidebar", "children": [] }
                ]},
                { "id": 4, "kind": "View", "test_id": "content-root", "children": [
                    { "id": 5, "kind": "Text", "label": "Body", "children": [] }
                ]}
            ]}
        ]));
        let found = subtree_by_test_id(&tree, "content-root").expect("anchor present");
        assert_eq!(found.id, 4);
        assert_eq!(found.children[0].id, 5);
        assert!(subtree_by_test_id(&tree, "nope").is_none());

        // Aligning the scoped subtree pairs the content, never the chrome.
        let al = align(std::slice::from_ref(found), std::slice::from_ref(found));
        assert!(al.pairs.iter().any(|p| p.id_a == 5), "body aligned");
        assert!(!al.pairs.iter().any(|p| p.id_a == 3), "sidebar excluded");
    }

    #[test]
    fn unmatched_root_count_is_reported_both_ways() {
        // A has 2 roots, B has 1 — the extra root surfaces as unmatched-in-A.
        let a = snap(serde_json::json!([
            { "id": 1, "kind": "View", "children": [] },
            { "id": 2, "kind": "Overlay", "children": [] }
        ]));
        let b = snap(serde_json::json!([{ "id": 1, "kind": "View", "children": [] }]));
        let al = align(&a, &b);
        assert_eq!(matched(&al, 1), Some(1));
        assert_eq!(al.unmatched.len(), 1);
        assert!(al.unmatched[0].in_a && al.unmatched[0].kind == "Overlay");
    }
}
