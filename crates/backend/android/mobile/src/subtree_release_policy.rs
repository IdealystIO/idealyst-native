//! Which registered views a discarded subtree frees.
//!
//! `AndroidBackend::view_to_layout` holds a `GlobalRef` and a Taffy node for
//! every view the backend created. The `GlobalRef` is what keeps the Java
//! view alive, so an entry that outlives its subtree is a view the JVM can
//! never collect, and one more frame the layout pass writes every time it
//! runs. `Host::release_subtree` (a reactive branch swapped out, a keyed row
//! removed, a screen popped) and a portal's release both end here.
//!
//! The subtree is found through the LOGICAL layout tree, not the Java view
//! tree: entries are keyed by the address of one global reference, and a
//! `getChildAt` walk only yields fresh local references whose addresses
//! match nothing. The logical tree mirrors every `insert` the backend made,
//! reactive anchors (`display: contents`) included, which the flat Taffy
//! children skip.
//!
//! Un-gated so the regression tests run on the host; the JNI half is
//! `AndroidBackend::drain_released_subtrees` in `imp/mod.rs`.

use std::collections::{HashMap, HashSet};

use runtime_layout::{LayoutNode, LayoutTree};

/// The view keys to free for `roots`, each named once, in root-first order.
///
/// `key_of` maps a registered Taffy node back to its view key. A node that
/// maps to nothing (already freed, or never a view) is walked through but
/// yields no key.
///
/// `owned_elsewhere` names keys whose teardown belongs to another path: a
/// portal's overlay or a detached window root is an orphan Taffy root that
/// `release_portal` / `release_private_layer_window` free in order. The walk
/// neither frees such a node nor descends into it. (Neither is ever a Taffy
/// child of an in-flow parent, since `insert` skips them, so this matters
/// only when one is itself queued as a root.)
///
/// Naming each key once is load-bearing: `LayoutTree::remove_node` must not
/// run twice on one node, and two queued roots can overlap (a branch inside a
/// row that a keyed list then removes).
pub(crate) fn release_plan(
    tree: &LayoutTree,
    roots: &[LayoutNode],
    key_of: &HashMap<LayoutNode, usize>,
    owned_elsewhere: impl Fn(usize) -> bool,
) -> Vec<usize> {
    let mut plan = Vec::new();
    let mut seen: HashSet<LayoutNode> = HashSet::new();
    let mut stack: Vec<LayoutNode> = roots.iter().rev().copied().collect();
    while let Some(node) = stack.pop() {
        if !seen.insert(node) || tree.is_dropped(node) {
            continue;
        }
        if let Some(&key) = key_of.get(&node) {
            if owned_elsewhere(key) {
                continue;
            }
            plan.push(key);
        }
        stack.extend(tree.logical_children_of(node).into_iter().rev());
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tree shaped like the reported leak, `match` → view → keyed rows of
    /// text, with a reactive anchor between the branch root and its rows.
    /// Returns the tree, the key map and the nodes by name.
    fn roster() -> (LayoutTree, HashMap<LayoutNode, usize>, HashMap<&'static str, LayoutNode>) {
        let mut tree = LayoutTree::new();
        let app = tree.new_node();
        let rail = tree.new_node();
        let anchor = tree.new_contents_node();
        let row_a = tree.new_node();
        let text_a = tree.new_node();
        let row_b = tree.new_node();
        let text_b = tree.new_node();
        let nodes: HashMap<&'static str, LayoutNode> = [
            ("app", app),
            ("rail", rail),
            ("anchor", anchor),
            ("row_a", row_a),
            ("text_a", text_a),
            ("row_b", row_b),
            ("text_b", text_b),
        ]
        .into_iter()
        .collect();
        tree.add_child(app, rail);
        tree.add_child(rail, anchor);
        tree.add_child(anchor, row_a);
        tree.add_child(row_a, text_a);
        tree.add_child(anchor, row_b);
        tree.add_child(row_b, text_b);
        let key_of = nodes
            .iter()
            .enumerate()
            .map(|(i, (_, n))| (*n, 100 + i))
            .collect();
        (tree, key_of, nodes)
    }

    fn keys(key_of: &HashMap<LayoutNode, usize>, nodes: &HashMap<&str, LayoutNode>, names: &[&str]) -> HashSet<usize> {
        names.iter().map(|n| key_of[&nodes[n]]).collect()
    }

    #[test]
    fn regression_android_rebuild_frees_the_whole_discarded_subtree() {
        // The reported leak: each rebuild of the rail dropped the Rust
        // subtree but kept every view's `view_to_layout` entry, 16k views
        // after 2.6 days. Discarding the rail must free it, the anchor
        // under it and every row and text inside the anchor.
        let (tree, key_of, nodes) = roster();
        let plan = release_plan(&tree, &[nodes["rail"]], &key_of, |_| false);
        assert_eq!(
            plan.iter().copied().collect::<HashSet<_>>(),
            keys(&key_of, &nodes, &["rail", "anchor", "row_a", "text_a", "row_b", "text_b"]),
        );
        assert_eq!(plan.len(), 6, "each key once: {plan:?}");
        assert_eq!(plan[0], key_of[&nodes["rail"]], "root first");
        assert!(!plan.contains(&key_of[&nodes["app"]]), "the parent stays");
    }

    #[test]
    fn overlapping_roots_name_each_key_once() {
        // A keyed list removes a row whose inner branch was ALSO queued.
        let (tree, key_of, nodes) = roster();
        let plan = release_plan(&tree, &[nodes["text_a"], nodes["row_a"], nodes["rail"]], &key_of, |_| false);
        assert_eq!(plan.len(), 6, "{plan:?}");
    }

    #[test]
    fn a_portal_root_is_left_to_its_own_release() {
        let (tree, key_of, nodes) = roster();
        let row_b = key_of[&nodes["row_b"]];
        let plan = release_plan(&tree, &[nodes["rail"]], &key_of, |k| k == row_b);
        assert_eq!(
            plan.iter().copied().collect::<HashSet<_>>(),
            keys(&key_of, &nodes, &["rail", "anchor", "row_a", "text_a"]),
            "neither the owned node nor anything under it",
        );
    }

    #[test]
    fn a_freed_node_is_skipped() {
        let (mut tree, key_of, nodes) = roster();
        tree.remove_node(nodes["row_a"]);
        let plan = release_plan(&tree, &[nodes["row_a"]], &key_of, |_| false);
        assert!(plan.is_empty(), "{plan:?}");
    }
}
