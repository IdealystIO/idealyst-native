//! A layout-transparent node (`new_contents_node`) mounted as the app root.
//!
//! An app whose root is several nodes, or a reactive `if` / `match` / `for`,
//! hands the backend an anchor, which has no box: with no ancestor for its
//! children to join, they were laid out by nobody on macOS and Linux (an
//! empty window) and each as a full-window root, on top of one another, on
//! iOS, Android and Windows. CSS computes `display: contents` on the root
//! element to a box for the same reason; `blockify_root` does it here.

use runtime_layout::{LayoutNode, LayoutTree};
use runtime_shared::{Length, Position, StyleRules};

const W: f32 = 400.0;
const H: f32 = 300.0;

fn px(v: f32) -> Option<runtime_shared::Tokenized<Length>> {
    Some(Length::Px(v).into())
}

fn node(tree: &mut LayoutTree, rules: StyleRules) -> LayoutNode {
    let n = tree.new_node();
    tree.set_author_style(n, &rules);
    n
}

fn tall(tree: &mut LayoutTree, h: f32) -> LayoutNode {
    node(tree, StyleRules { height: px(h), ..Default::default() })
}

fn frame(tree: &LayoutTree, n: LayoutNode) -> (f32, f32, f32, f32) {
    let f = tree.frame_of(n);
    (f.x, f.y, f.width, f.height)
}

/// An anchor holding `children`, blockified and laid out.
fn mounted(tree: &mut LayoutTree, children: &[LayoutNode]) -> LayoutNode {
    let anchor = tree.new_contents_node();
    for c in children {
        tree.add_child(anchor, *c);
    }
    tree.blockify_root(anchor);
    tree.compute(anchor, W, H);
    anchor
}

#[test]
fn regression_several_root_nodes_stack_in_the_window() {
    let mut tree = LayoutTree::new();
    let (a, b) = (tall(&mut tree, 40.0), tall(&mut tree, 50.0));
    let root = mounted(&mut tree, &[a, b]);
    assert!(tree.is_root(root), "the backends that lay out every root reach it");
    assert!(!tree.is_root(a) && !tree.is_root(b));
    assert_eq!(frame(&tree, root), (0.0, 0.0, W, H));
    assert_eq!(frame(&tree, a), (0.0, 0.0, W, 40.0));
    assert_eq!(frame(&tree, b), (0.0, 40.0, W, 50.0));
}

/// A lone child fills the window, as the app root it stands in for does:
/// a root `if` whose branch is an unsized view.
#[test]
fn regression_a_lone_root_child_fills_the_window() {
    let mut tree = LayoutTree::new();
    let branch = node(&mut tree, StyleRules::default());
    let inner = tall(&mut tree, 20.0);
    tree.add_child(branch, inner);
    let root = mounted(&mut tree, &[branch]);
    assert_eq!(frame(&tree, branch), (0.0, 0.0, W, H));
    assert_eq!(frame(&tree, inner), (0.0, 0.0, W, 20.0));
    // A later, smaller viewport.
    tree.compute(root, 200.0, 100.0);
    assert_eq!(frame(&tree, branch), (0.0, 0.0, 200.0, 100.0));
}

/// Content taller than the window does not grow the lone child past it:
/// a scroller inside has to be bounded.
#[test]
fn a_lone_root_child_is_bounded_by_the_window() {
    let mut tree = LayoutTree::new();
    let branch = node(&mut tree, StyleRules::default());
    let scroller = tree.new_node();
    tree.set_overflow_scroll(scroller, false);
    let content = tall(&mut tree, 5000.0);
    tree.add_child(scroller, content);
    tree.add_child(branch, scroller);
    mounted(&mut tree, &[branch]);
    assert_eq!(frame(&tree, branch), (0.0, 0.0, W, H));
    assert_eq!(frame(&tree, scroller), (0.0, 0.0, W, H));
}

/// A layout pass that ran before the mount (iOS and Android lay out every
/// root whenever one is scheduled) sized each child as a window; mounting
/// undoes it.
#[test]
fn a_pass_before_the_mount_leaves_no_size_behind() {
    let mut tree = LayoutTree::new();
    let (a, b) = (node(&mut tree, StyleRules::default()), tall(&mut tree, 50.0));
    let inner = tall(&mut tree, 20.0);
    tree.add_child(a, inner);
    let anchor = tree.new_contents_node();
    tree.add_child(anchor, a);
    tree.add_child(anchor, b);
    tree.compute(a, W, H);
    tree.compute(b, W, H);
    tree.blockify_root(anchor);
    tree.compute(anchor, W, H);
    assert_eq!(frame(&tree, a), (0.0, 0.0, W, 20.0));
    assert_eq!(frame(&tree, b), (0.0, 20.0, W, 50.0));
}

/// A lone child keeps a size of its own.
#[test]
fn a_lone_root_child_keeps_its_own_size() {
    let mut tree = LayoutTree::new();
    let card = node(&mut tree, StyleRules { width: px(100.0), height: px(60.0), ..Default::default() });
    mounted(&mut tree, &[card]);
    assert_eq!(frame(&tree, card), (0.0, 0.0, 100.0, 60.0));
}

/// An absolute sibling (a badge, a toast strip) is not a second in-flow
/// node: the page still fills.
#[test]
fn an_absolute_sibling_leaves_the_lone_child_filling() {
    let mut tree = LayoutTree::new();
    let page = node(&mut tree, StyleRules::default());
    let badge = node(
        &mut tree,
        StyleRules {
            position: Some(Position::Absolute),
            right: px(10.0),
            bottom: px(10.0),
            width: px(20.0),
            height: px(20.0),
            ..Default::default()
        },
    );
    mounted(&mut tree, &[page, badge]);
    assert_eq!(frame(&tree, page), (0.0, 0.0, W, H));
    assert_eq!(frame(&tree, badge), (W - 30.0, H - 30.0, 20.0, 20.0));
}

/// The branch swaps under the mounted anchor, from one node to two and
/// back, through a nested anchor as a nested `if` mounts.
#[test]
fn a_swapped_branch_is_laid_out() {
    let mut tree = LayoutTree::new();
    let first = node(&mut tree, StyleRules::default());
    let root = mounted(&mut tree, &[first]);

    tree.remove_child(root, first);
    tree.remove_node(first);
    let nested = tree.new_contents_node();
    let (a, b) = (tall(&mut tree, 30.0), tall(&mut tree, 70.0));
    tree.add_child(nested, a);
    tree.add_child(nested, b);
    tree.add_child(root, nested);
    tree.compute(root, W, H);
    assert_eq!(frame(&tree, a), (0.0, 0.0, W, 30.0));
    assert_eq!(frame(&tree, b), (0.0, 30.0, W, 70.0));

    tree.remove_child(root, nested);
    for n in [a, b, nested] {
        tree.remove_node(n);
    }
    let last = node(&mut tree, StyleRules::default());
    tree.add_child(root, last);
    tree.compute(root, W, H);
    assert_eq!(frame(&tree, last), (0.0, 0.0, W, H));
}

/// Mounting again (a dev reconnect replays the mount) changes nothing, an
/// ordinary root is left alone, and an anchor inside a tree stays
/// transparent.
#[test]
fn only_an_unattached_anchor_becomes_a_box() {
    let mut tree = LayoutTree::new();
    let (a, b) = (tall(&mut tree, 40.0), tall(&mut tree, 50.0));
    let root = mounted(&mut tree, &[a, b]);
    tree.blockify_root(root);
    tree.compute(root, W, H);
    assert_eq!(frame(&tree, b), (0.0, 40.0, W, 50.0));

    let view = node(&mut tree, StyleRules::default());
    let anchor = tree.new_contents_node();
    let child = tall(&mut tree, 10.0);
    tree.add_child(anchor, child);
    tree.add_child(view, anchor);
    tree.blockify_root(view);
    tree.blockify_root(anchor);
    assert!(tree.is_contents(anchor));
    tree.compute(view, W, H);
    assert_eq!(frame(&tree, child), (0.0, 0.0, W, 10.0), "not stretched: the view is an author's own box");
}

/// A removed root frees its slot and leaves nothing behind.
#[test]
fn a_removed_anchor_root_frees_its_node() {
    let mut tree = LayoutTree::new();
    let a = tall(&mut tree, 40.0);
    let root = mounted(&mut tree, &[a]);
    tree.remove_child(root, a);
    tree.remove_node(a);
    tree.remove_node(root);
    assert_eq!(tree.live_node_count(), 0);
}
