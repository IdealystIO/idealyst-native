//! The target app's state as the Inspector shows it: the wire types
//! (re-exported from `inspector-protocol`, which the server fills in) plus
//! the pure derivations the screens render (the component tree's visible
//! rows, a component's breadcrumb, display formatting).
//!
//! No UI and no transport in here.

use std::collections::HashSet;

pub use inspector_protocol::model::*;
pub use inspector_protocol::{AppInfo, Focus};

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
    /// `element` is the element the component renders as — what a hover
    /// highlights in the app.
    Component { instance_id: u64, name: String, element: u64 },
    Element { id: u64, kind: String },
}

impl TreeRow {
    /// The element this row stands for in the app: an element row's own,
    /// a component row's rendered element.
    pub fn element_id(&self) -> u64 {
        match self.kind {
            RowKind::Component { element, .. } => element,
            RowKind::Element { id, .. } => id,
        }
    }
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
                kind: RowKind::Component { instance_id: c.instance_id, name: c.name.clone(), element: e.id },
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

    /// Hover-to-highlight boxes the row's element: a component row's is the
    /// element it renders as (shared by nested components).
    #[test]
    fn rows_know_which_element_to_highlight() {
        let collapsed = HashSet::new();
        let rows = visible_rows(&sample(), &TreeOptions { show_elements: true, collapsed: &collapsed, filter: "" });
        let ids: Vec<u64> = rows.iter().map(TreeRow::element_id).collect();
        assert_eq!(ids, [1, 1, 2, 2, 3, 3, 3, 4]);
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
    fn formatting() {
        assert_eq!(ago(2_400), "2 s ago");
        assert_eq!(micros(38_200), "38.2 ms");
        assert_eq!(micros(412), "412 µs");
        assert_eq!(truncate("abcdef", 3), "abc…");
    }
}
