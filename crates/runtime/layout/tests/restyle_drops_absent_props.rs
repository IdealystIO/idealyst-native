//! `LayoutTree::set_author_style` takes a node's COMPLETE declared style. A
//! property the previous apply declared and this one leaves out goes back
//! to what the node had before any style declared it — the framework seed
//! (`new_node`'s flex column, a scroller's fill, an intrinsic floor) — the
//! way a browser drops a CSS declaration when its class goes away.
//!
//! The case that found it: a stylesheet variant `off { width: 0, height:
//! 0, overflow: Hidden }` / `on {}`. Flipping `off` → `on` re-applies the
//! `on` rules, which say nothing about size. Web dropped the `off` class
//! and the slot sized to its content again; every native backend kept the
//! 0×0 Taffy size, so the slot (and the button in it) stayed collapsed.

use runtime_layout::{Frame, LayoutNode, LayoutTree};
use runtime_shared::{FlexDirection, Length, Overflow, StyleRules, Tokenized};

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Length::Px(v).into())
}

fn num(v: f32) -> Option<Tokenized<f32>> {
    Some(v.into())
}

/// A 400×300 column root holding `slot`, which holds a 40×20 box.
fn slot_tree() -> (LayoutTree, LayoutNode, LayoutNode) {
    let mut tree = LayoutTree::new();
    let root = tree.new_node();
    tree.set_author_style(root, &StyleRules { width: px(400.0), height: px(300.0), ..Default::default() });
    let slot = tree.new_node();
    let content = tree.new_node();
    tree.set_author_style(content, &StyleRules { width: px(40.0), height: px(20.0), ..Default::default() });
    tree.add_child(root, slot);
    tree.add_child(slot, content);
    (tree, root, slot)
}

fn layout(tree: &mut LayoutTree, root: LayoutNode, node: LayoutNode) -> Frame {
    tree.compute(root, 400.0, 300.0);
    tree.frame_of(node)
}

#[test]
fn regression_leaving_a_collapsing_variant_restores_the_slot_size() {
    let (mut tree, root, slot) = slot_tree();
    let on = StyleRules::default();
    let off = StyleRules {
        width: px(0.0),
        height: px(0.0),
        overflow: Some(Overflow::Hidden),
        ..Default::default()
    };

    tree.set_author_style(slot, &on);
    let initial = layout(&mut tree, root, slot);
    assert_eq!((initial.width, initial.height), (400.0, 20.0), "stretched width, content height");

    tree.set_author_style(slot, &off);
    let collapsed = layout(&mut tree, root, slot);
    assert_eq!((collapsed.width, collapsed.height), (0.0, 0.0));

    assert!(tree.set_author_style(slot, &on), "dropping width/height is a geometry change");
    let restored = layout(&mut tree, root, slot);
    assert_eq!(
        (restored.width, restored.height),
        (initial.width, initial.height),
        "`on` declares no size, so the slot sizes as it did before `off`"
    );

    // And again: the second round trip must not have lost the seed.
    tree.set_author_style(slot, &off);
    layout(&mut tree, root, slot);
    tree.set_author_style(slot, &on);
    let again = layout(&mut tree, root, slot);
    assert_eq!((again.width, again.height), (initial.width, initial.height));
}

#[test]
fn regression_a_dropped_width_drops_its_min_width_floor() {
    // A declared px width also floors `min-width` (an explicitly sized box
    // does not shrink below its size). The floor belongs to the width: once
    // the width is gone the box shrinks to its content in a tight row again.
    let mut tree = LayoutTree::new();
    let root = tree.new_node();
    tree.set_author_style(
        root,
        &StyleRules {
            width: px(100.0),
            height: px(50.0),
            flex_direction: Some(FlexDirection::Row),
            ..Default::default()
        },
    );
    let item = tree.new_node();
    let content = tree.new_node();
    tree.set_author_style(content, &StyleRules { width: px(30.0), height: px(10.0), ..Default::default() });
    tree.add_child(root, item);
    tree.add_child(item, content);

    tree.set_author_style(item, &StyleRules { width: px(300.0), ..Default::default() });
    tree.compute(root, 100.0, 50.0);
    assert_eq!(tree.frame_of(item).width, 300.0);

    tree.set_author_style(item, &StyleRules::default());
    tree.compute(root, 100.0, 50.0);
    assert_eq!(tree.frame_of(item).width, 30.0, "content width, no leftover floor");
}

#[test]
fn regression_dropped_box_props_revert_to_their_seed() {
    // Margin, padding, flex direction and max-width each revert when the
    // next apply leaves them out.
    let (mut tree, root, slot) = slot_tree();
    let base = layout(&mut tree, root, slot);

    tree.set_author_style(
        slot,
        &StyleRules {
            margin_left: px(10.0),
            margin_top: px(5.0),
            padding_top: px(7.0),
            padding_left: px(3.0),
            max_width: px(120.0),
            flex_direction: Some(FlexDirection::Row),
            ..Default::default()
        },
    );
    let styled = layout(&mut tree, root, slot);
    assert_eq!((styled.x, styled.y, styled.width), (10.0, 5.0, 120.0));

    tree.set_author_style(slot, &StyleRules::default());
    let reverted = layout(&mut tree, root, slot);
    assert_eq!(reverted, base, "margin, padding and max-width all dropped");
}

#[test]
fn a_scroll_viewports_fill_seed_survives_an_author_override_being_dropped() {
    // The seed a property reverts to is whatever the node had before the
    // style declared it: a scroll viewport's `flex_grow: 1` fill, not the
    // CSS initial `0`.
    let mut tree = LayoutTree::new();
    let root = tree.new_node();
    tree.set_author_style(root, &StyleRules { width: px(200.0), height: px(400.0), ..Default::default() });
    let viewport = tree.new_node();
    tree.set_overflow_scroll(viewport, false);
    tree.add_child(root, viewport);

    tree.compute(root, 200.0, 400.0);
    assert_eq!(tree.frame_of(viewport).height, 400.0, "seeded viewport fills");

    tree.set_author_style(
        viewport,
        &StyleRules { flex_grow: num(0.0), flex_basis: px(50.0), ..Default::default() },
    );
    tree.compute(root, 200.0, 400.0);
    assert_eq!(tree.frame_of(viewport).height, 50.0);

    tree.set_author_style(viewport, &StyleRules::default());
    tree.compute(root, 200.0, 400.0);
    assert_eq!(tree.frame_of(viewport).height, 400.0, "back to the fill seed");
}

#[test]
fn properties_still_declared_are_kept_across_applies() {
    // Re-applying the same rules, or rules that keep a property, keeps it.
    let (mut tree, root, slot) = slot_tree();
    let rules = StyleRules { width: px(90.0), height: px(30.0), ..Default::default() };
    tree.set_author_style(slot, &rules);
    tree.set_author_style(slot, &rules);
    tree.set_author_style(slot, &StyleRules { width: px(90.0), ..Default::default() });
    let f = layout(&mut tree, root, slot);
    assert_eq!((f.width, f.height), (90.0, 20.0), "width kept, height dropped to content");
}
