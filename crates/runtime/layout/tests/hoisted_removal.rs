//! Removing an absolutely positioned box must not leave its key in a
//! containing block's hoisted list.
//!
//! Taffy lays out-of-flow boxes out from their containing block and records
//! them there (`hoisted_children`); `round_layout` walks that list. The list
//! is rebuilt only when the containing block lays out as a container
//! (`compute_oof_layout` returns early for a leaf). So a containing block
//! whose last child is the removed box keeps the freed key, and the next
//! `compute` panicked with "invalid SlotMap key used": a process abort on
//! the native backends, which run layout behind a non-unwinding boundary.
//! Seen as a Kanban board's landing card (absolute, in an absolute layer)
//! crashing macOS when the card's 220 ms glide ended.

use runtime_layout::LayoutTree;
use runtime_shared::{Length, Position, StyleRules};

const W: f32 = 400.0;
const H: f32 = 300.0;

fn px(v: f32) -> Option<runtime_shared::Tokenized<Length>> {
    Some(Length::Px(v).into())
}

/// A stage, an absolute layer pinned to it, and one absolute card in the
/// layer: the layer is the card's containing block. Returns
/// `(tree, stage, layer, card)`, laid out once.
fn board() -> (LayoutTree, runtime_layout::LayoutNode, runtime_layout::LayoutNode, runtime_layout::LayoutNode) {
    let mut tree = LayoutTree::new();
    let stage = tree.new_node();
    tree.set_style(
        stage,
        &StyleRules {
            position: Some(Position::Relative),
            width: px(W),
            height: px(H),
            ..Default::default()
        },
    );
    let layer = tree.new_node();
    tree.set_style(
        layer,
        &StyleRules {
            position: Some(Position::Absolute),
            top: px(0.0),
            left: px(0.0),
            right: px(0.0),
            bottom: px(0.0),
            ..Default::default()
        },
    );
    let card = tree.new_node();
    tree.set_style(
        card,
        &StyleRules {
            position: Some(Position::Absolute),
            left: px(20.0),
            top: px(30.0),
            width: px(100.0),
            height: px(40.0),
            ..Default::default()
        },
    );
    tree.add_child(layer, card);
    tree.add_child(stage, layer);
    tree.compute(stage, W, H);
    let f = tree.frame_of(card);
    assert_eq!((f.x, f.y, f.width, f.height), (20.0, 30.0, 100.0, 40.0), "the card lays out in its layer");
    (tree, stage, layer, card)
}

/// The last child of a containing block, removed the way a backend
/// unmounts a keyed row (`remove_child`, then `remove_node`), then a
/// relayout: no panic.
#[test]
fn removing_the_last_absolute_child_then_relaying_out_does_not_panic() {
    let (mut tree, stage, layer, card) = board();
    tree.remove_child(layer, card);
    tree.remove_node(card);
    tree.mark_dirty(layer);
    tree.compute(stage, W, H);
    let f = tree.frame_of(layer);
    assert_eq!((f.width, f.height), (W, H), "the emptied layer still fills the stage");
}

/// The node held back for the stale list is given back once the containing
/// block lays out with children again: nothing leaks.
#[test]
fn the_held_node_is_freed_once_the_containing_block_relays_out_with_children() {
    let (mut tree, stage, layer, card) = board();
    let before = tree.live_node_count();
    tree.remove_child(layer, card);
    tree.remove_node(card);
    tree.compute(stage, W, H);

    // A new card: the layer lays out as a container again and rebuilds
    // its hoisted list without the old one.
    let next = tree.new_node();
    tree.set_style(
        next,
        &StyleRules {
            position: Some(Position::Absolute),
            left: px(5.0),
            top: px(5.0),
            width: px(10.0),
            height: px(10.0),
            ..Default::default()
        },
    );
    tree.add_child(layer, next);
    tree.compute(stage, W, H);
    assert_eq!(tree.live_node_count(), before, "the removed card's slot is freed (one card out, one in)");
    let f = tree.frame_of(next);
    assert_eq!((f.x, f.y), (5.0, 5.0));
}

/// Removing the containing block itself frees a node it was holding, once
/// the block's own containing block has rebuilt its list (here the stage
/// keeps an in-flow child, so it lays out as a container).
#[test]
fn the_held_node_is_freed_with_its_containing_block() {
    let (mut tree, stage, layer, card) = board();
    let body = tree.new_node();
    tree.set_style(body, &StyleRules { height: px(10.0), ..Default::default() });
    tree.add_child(stage, body);
    tree.compute(stage, W, H);
    tree.remove_child(layer, card);
    tree.remove_node(card);
    tree.compute(stage, W, H);
    tree.remove_child(stage, layer);
    tree.remove_node(layer);
    tree.compute(stage, W, H);
    assert_eq!(tree.live_node_count(), 2, "only the stage and its body are left");
}

/// A containing block emptied of everything, then removed itself while its
/// own block is a leaf too: every stale list is still walkable.
#[test]
fn nested_stale_lists_stay_walkable() {
    let (mut tree, stage, layer, card) = board();
    tree.remove_child(layer, card);
    tree.remove_node(card);
    tree.compute(stage, W, H);
    tree.remove_child(stage, layer);
    tree.remove_node(layer);
    tree.compute(stage, W, H);
    tree.compute(stage, W, H);
    let f = tree.frame_of(stage);
    assert_eq!((f.width, f.height), (W, H));
}

fn relative(tree: &mut LayoutTree) -> runtime_layout::LayoutNode {
    let n = tree.new_node();
    tree.set_style(
        n,
        &StyleRules {
            position: Some(Position::Relative),
            width: px(50.0),
            height: px(50.0),
            ..Default::default()
        },
    );
    n
}

fn pinned(tree: &mut LayoutTree) -> runtime_layout::LayoutNode {
    let n = tree.new_node();
    tree.set_style(
        n,
        &StyleRules {
            position: Some(Position::Absolute),
            left: px(1.0),
            top: px(1.0),
            width: px(5.0),
            height: px(5.0),
            ..Default::default()
        },
    );
    n
}

/// A root holding a block, the block holding a static wrapper around an
/// absolute box. Returns `(tree, root, block, wrapper, pin)`, laid out once.
fn wrapped() -> (
    LayoutTree,
    runtime_layout::LayoutNode,
    runtime_layout::LayoutNode,
    runtime_layout::LayoutNode,
    runtime_layout::LayoutNode,
) {
    let mut tree = LayoutTree::new();
    let root = relative(&mut tree);
    let block = relative(&mut tree);
    let wrapper = tree.new_node();
    let pin = pinned(&mut tree);
    tree.add_child(root, block);
    tree.add_child(block, wrapper);
    tree.add_child(wrapper, pin);
    tree.compute(root, W, H);
    (tree, root, block, wrapper, pin)
}

/// The box outlives its removed parent by a layout pass: by the time it is
/// removed, nothing connects it to the block that still lists it.
#[test]
fn removing_the_box_a_pass_after_its_parent_does_not_panic() {
    let (mut tree, root, block, wrapper, pin) = wrapped();
    tree.remove_child(block, wrapper);
    tree.remove_node(wrapper);
    tree.compute(root, W, H);
    tree.remove_node(pin);
    tree.mark_dirty(block);
    tree.compute(root, W, H);
}

/// The pass in between is another root's (a portal, a virtualizer row).
#[test]
fn another_roots_pass_between_the_removals_does_not_panic() {
    let (mut tree, root, block, wrapper, pin) = wrapped();
    let other = tree.new_node();
    tree.remove_child(block, wrapper);
    tree.remove_node(wrapper);
    tree.compute(other, W, H);
    tree.remove_node(pin);
    tree.compute(root, W, H);
}

/// The box moved to a second block before it was removed: the first block,
/// emptied by the move, still lists it.
#[test]
fn removing_a_box_that_moved_between_blocks_does_not_panic() {
    let mut tree = LayoutTree::new();
    let root = relative(&mut tree);
    let (a, b) = (relative(&mut tree), relative(&mut tree));
    let pin = pinned(&mut tree);
    tree.add_child(root, a);
    tree.add_child(root, b);
    tree.add_child(a, pin);
    tree.compute(root, W, H);
    tree.remove_child(a, pin);
    tree.add_child(b, pin);
    tree.compute(root, W, H);
    tree.remove_child(b, pin);
    tree.remove_node(pin);
    tree.compute(root, W, H);
    // `b` lays out as a container again and drops the pin; `a` has not.
    let next = pinned(&mut tree);
    tree.add_child(b, next);
    tree.compute(root, W, H);
    tree.compute(root, W, H);
    assert_eq!(tree.frame_of(next).width, 5.0);
}

/// Emptying a list block by block frees everything in the end.
#[test]
fn nothing_is_held_once_the_whole_tree_is_removed() {
    let (mut tree, root, block, wrapper, pin) = wrapped();
    for (parent, child) in [(wrapper, pin), (block, wrapper), (root, block)] {
        tree.remove_child(parent, child);
        tree.remove_node(child);
        tree.compute(root, W, H);
    }
    tree.remove_node(root);
    assert_eq!(tree.live_node_count(), 0);
}
