//! `runtime_shared::introspect` — the platform-native introspection
//! data model and the boundary-pruning tree walk.
//!
//! RELOCATED from `runtime-core/tests/native_introspect.rs` (deletion
//! baseline §4.2, SV-R). `shared/src/introspect.rs` had **zero** tests of
//! its own, so these four were the module's ONLY coverage and would have
//! been deleted along with their host crate.
//!
//! Two layers, matching the two risk areas:
//! 1. The serialized data model is stable and round-trips (the external
//!    cross-platform parity harness depends on the exact JSON shape).
//! 2. The boundary-pruning tree walk — the one piece of non-trivial logic
//!    every real backend shares — is correct in isolation.
//!
//! **Deliberately NOT relocated** (they are DIES-legit, not SV-R): the
//! old file's third layer, `robot_introspect_native_routes_through_backend_closure`
//! and `ui_macro_view_test_id_registers`. Both drove the OLD walker's
//! registry wiring (`TestRuntime` + the walker-attached introspection
//! closure + old-core `ui!` emission), none of which survives. Their
//! successor on this core is the vocabulary robot registry — the
//! closure-over-`IntrospectionOps` wiring in
//! `runtime-vocabulary/src/robot.rs`, exercised by that crate's
//! `--features robot` suite — not this module.

use runtime_shared::introspect::{
    collect_native_tree, keys, NativeNode, NativeRect, NativeValue,
};

fn rect() -> NativeRect {
    NativeRect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 }
}

// ---------------------------------------------------------------------------
// 1. Data model: serde round-trip + the documented `{type,value}` shape.
// ---------------------------------------------------------------------------

#[test]
fn native_value_serializes_as_tagged_type_value() {
    let v = NativeValue::Color([0.5, 0.25, 0.0, 1.0]);
    let json = serde_json::to_value(&v).unwrap();
    assert_eq!(json["type"], "color");
    assert_eq!(json["value"][0], 0.5);

    assert_eq!(serde_json::to_value(NativeValue::Length(12.0)).unwrap()["type"], "length");
    assert_eq!(serde_json::to_value(NativeValue::Number(400.0)).unwrap()["type"], "number");
    assert_eq!(serde_json::to_value(NativeValue::Text("hi".into())).unwrap()["type"], "text");
    assert_eq!(serde_json::to_value(NativeValue::Flag(true)).unwrap()["type"], "flag");
}

#[test]
fn native_node_round_trips_through_json_with_nested_children() {
    let mut child = NativeNode::leaf("inner", rect()).with_role("scroll_view.content");
    child.set(keys::TEXT, Some(NativeValue::Text("hello".into())));

    let mut grandchild = NativeNode::leaf("leaf", rect());
    grandchild.set(keys::FONT_SIZE, Some(NativeValue::Length(14.0)));
    child.children.push(grandchild);

    let mut root = NativeNode::leaf("NSView", rect());
    root.set(keys::BACKGROUND_COLOR, Some(NativeValue::Color([1.0, 0.0, 0.0, 1.0])));
    root.set(keys::CORNER_RADIUS, Some(NativeValue::Length(8.0)));
    root.set(keys::HIDDEN, Some(NativeValue::Flag(false)));
    root.children.push(child);

    let json = serde_json::to_string(&root).unwrap();
    let back: NativeNode = serde_json::from_str(&json).unwrap();
    assert_eq!(back, root, "NativeNode must survive a JSON round-trip unchanged");

    // 2-level hierarchy preserved.
    assert_eq!(back.children[0].children[0].class, "leaf");
    assert_eq!(back.children[0].role.as_deref(), Some("scroll_view.content"));
}

#[test]
fn empty_props_and_children_are_omitted_from_json() {
    // A bare leaf should not emit empty `props`/`children`/`role` keys — keeps
    // the parity payload compact and the diff free of spurious empties.
    let json = serde_json::to_value(NativeNode::leaf("div", rect())).unwrap();
    assert!(json.get("props").is_none());
    assert!(json.get("children").is_none());
    assert!(json.get("role").is_none());
    assert!(json.get("frame").is_some());
}

// ---------------------------------------------------------------------------
// 2. Boundary walk: descend platform internals, prune at framework roots.
// ---------------------------------------------------------------------------

#[test]
fn collect_native_tree_prunes_at_framework_boundaries() {
    // Synthetic adjacency. `H = usize` indexes into `kids`.
    //   0 ─┬─ 1 (framework root → pruned; its child 3 never visited)
    //      └─ 2 (platform internal → kept)
    //            └─ 4 (framework root → pruned)
    // The root (0) is always read even though boundaries are checked on
    // descendants only.
    let kids: Vec<Vec<usize>> = vec![
        vec![1, 2], // 0
        vec![3],    // 1
        vec![4],    // 2
        vec![],     // 3
        vec![],     // 4
    ];
    let boundaries = [1usize, 4];

    let read = |i: &usize| NativeNode::leaf(format!("n{i}"), rect());
    let children = |i: &usize| kids[*i].clone();
    let is_boundary = |i: &usize| boundaries.contains(i);

    let tree = collect_native_tree(&0usize, &read, &children, &is_boundary);

    assert_eq!(tree.class, "n0");
    // 1 pruned (boundary), 2 kept.
    assert_eq!(tree.children.len(), 1);
    assert_eq!(tree.children[0].class, "n2");
    // 4 pruned under 2 → 2 has no children, and 3 (under the pruned 1) never
    // appears anywhere.
    assert!(tree.children[0].children.is_empty());
    assert!(!contains_class(&tree, "n1"));
    assert!(!contains_class(&tree, "n3"));
    assert!(!contains_class(&tree, "n4"));
}

fn contains_class(node: &NativeNode, class: &str) -> bool {
    node.class == class || node.children.iter().any(|c| contains_class(c, class))
}
