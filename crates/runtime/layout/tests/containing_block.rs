//! An absolutely positioned box resolves against its containing block —
//! the nearest POSITIONED ancestor (`relative`, `absolute`, `sticky`, or a
//! transformed box) — not its direct parent, as in CSS. A node with no
//! `position` is `static` and is not a containing block.
//!
//! `frame_of` stays relative to the node's layout parent (`parent_of`),
//! because native backends nest each view inside its parent's view: a box
//! placed against an ancestor further up reports its offset translated
//! into its parent's coordinate space.

use runtime_layout::{LayoutNode, LayoutTree};
use runtime_shared::{DisplayKind, Length, Position, StyleRules, TrackSize, Transform};

fn px(v: f32) -> Option<runtime_shared::Tokenized<Length>> {
    Some(Length::Px(v).into())
}

fn pct(v: f32) -> Option<runtime_shared::Tokenized<Length>> {
    Some(Length::Percent(v).into())
}

fn styled(tree: &mut LayoutTree, rules: StyleRules) -> LayoutNode {
    let n = tree.new_node();
    tree.set_author_style(n, &rules);
    n
}

/// `position: absolute; top: 0; left: 0; bottom: 0; width: 228`.
fn pin_rules() -> StyleRules {
    StyleRules {
        position: Some(Position::Absolute),
        top: px(0.0),
        left: px(0.0),
        bottom: px(0.0),
        width: px(228.0),
        ..Default::default()
    }
}

/// The Timely board: a relative 100%×100% stage holding a full-size seat
/// and an UNSTYLED wrapper around an absolute pin. Returns
/// `(tree, root, stage, wrapper, pin)` laid out at 800×600.
fn board(wrapper_rules: StyleRules) -> (LayoutTree, LayoutNode, LayoutNode, LayoutNode, LayoutNode) {
    let mut tree = LayoutTree::new();
    let root = styled(&mut tree, StyleRules { width: px(800.0), height: px(600.0), ..Default::default() });
    let stage = styled(
        &mut tree,
        StyleRules {
            position: Some(Position::Relative),
            width: pct(100.0),
            height: pct(100.0),
            ..Default::default()
        },
    );
    let seat = styled(&mut tree, StyleRules { width: pct(100.0), height: pct(100.0), ..Default::default() });
    let wrapper = styled(&mut tree, wrapper_rules);
    let pin = styled(&mut tree, pin_rules());
    tree.add_child(root, stage);
    tree.add_child(stage, seat);
    tree.add_child(stage, wrapper);
    tree.add_child(wrapper, pin);
    tree.compute(root, 800.0, 600.0);
    (tree, root, stage, wrapper, pin)
}

/// Regression: an absolute pin inside an unstyled wrapper resolved against
/// the wrapper (0 tall, after a full-height sibling) and came out 0 tall
/// on native, while web stretched it to the positioned stage's height.
#[test]
fn regression_absolute_box_resolves_against_nearest_positioned_ancestor() {
    let (tree, _root, stage, wrapper, pin) = board(StyleRules::default());

    assert_eq!(tree.frame_of(stage).height, 600.0);
    let w = tree.frame_of(wrapper);
    assert_eq!((w.y, w.height), (600.0, 0.0), "the wrapper follows the full-height seat");

    let p = tree.frame_of(pin);
    assert_eq!((p.width, p.height), (228.0, 600.0), "the pin fills the STAGE's height");
    // Parent-relative: the stage's top is 600 above the wrapper.
    assert_eq!((p.x, p.y), (0.0, -600.0), "the pin sits at the stage's top-left");
}

/// A positioned wrapper IS the containing block: the pin resolves against it.
#[test]
fn relative_wrapper_is_the_containing_block() {
    let (tree, _root, _stage, _wrapper, pin) =
        board(StyleRules { position: Some(Position::Relative), ..Default::default() });
    let p = tree.frame_of(pin);
    assert_eq!((p.x, p.y, p.width, p.height), (0.0, 0.0, 228.0, 0.0));
}

/// `sticky` establishes a containing block, as in CSS.
#[test]
fn sticky_wrapper_is_the_containing_block() {
    let (tree, _root, _stage, _wrapper, pin) =
        board(StyleRules { position: Some(Position::Sticky), ..Default::default() });
    assert_eq!(tree.frame_of(pin).height, 0.0);
}

/// A transformed box establishes a containing block, as in CSS.
#[test]
fn transformed_wrapper_is_the_containing_block() {
    let (tree, _root, _stage, _wrapper, pin) =
        board(StyleRules { transform: Some(vec![Transform::TranslateX(Length::Px(0.0))]), ..Default::default() });
    assert_eq!(tree.frame_of(pin).height, 0.0);
}

/// With no positioned ancestor at all, the layout root is the containing
/// block.
#[test]
fn root_is_the_containing_block_when_nothing_is_positioned() {
    let mut tree = LayoutTree::new();
    let root = styled(&mut tree, StyleRules { width: px(800.0), height: px(600.0), ..Default::default() });
    let header = styled(&mut tree, StyleRules { height: px(50.0), ..Default::default() });
    let body = styled(&mut tree, StyleRules { padding_left: px(10.0), ..Default::default() });
    let wrapper = styled(&mut tree, StyleRules::default());
    let pin = styled(&mut tree, pin_rules());
    tree.add_child(root, header);
    tree.add_child(root, body);
    tree.add_child(body, wrapper);
    tree.add_child(wrapper, pin);
    tree.compute(root, 800.0, 600.0);

    let p = tree.frame_of(pin);
    assert_eq!((p.width, p.height), (228.0, 600.0));
    // The wrapper sits at (10, 50) in the root; the pin at the root's origin.
    assert_eq!((p.x, p.y), (-10.0, -50.0));
}

/// Offsets accumulate through several static ancestors, and an absolute
/// box with no insets keeps its static position (where it would sit in
/// flow), which needs no translation beyond its parent's.
#[test]
fn translation_spans_every_static_ancestor() {
    let mut tree = LayoutTree::new();
    let root = styled(&mut tree, StyleRules { width: px(800.0), height: px(600.0), ..Default::default() });
    let stage = styled(
        &mut tree,
        StyleRules { position: Some(Position::Relative), padding_top: px(5.0), ..Default::default() },
    );
    let a = styled(&mut tree, StyleRules { padding_left: px(7.0), ..Default::default() });
    let b = styled(&mut tree, StyleRules { padding_top: px(11.0), ..Default::default() });
    let pinned = styled(
        &mut tree,
        StyleRules {
            position: Some(Position::Absolute),
            top: px(1.0),
            left: px(2.0),
            width: px(10.0),
            height: px(10.0),
            ..Default::default()
        },
    );
    let floating = styled(
        &mut tree,
        StyleRules { position: Some(Position::Absolute), width: px(10.0), height: px(10.0), ..Default::default() },
    );
    tree.add_child(root, stage);
    tree.add_child(stage, a);
    tree.add_child(a, b);
    tree.add_child(b, pinned);
    tree.add_child(b, floating);
    tree.compute(root, 800.0, 600.0);

    // b's border box sits at (7, 5) in the stage.
    let p = tree.frame_of(pinned);
    assert_eq!((p.x, p.y), (2.0 - 7.0, 1.0 - 5.0));
    // No insets: the static position, inside b's padding.
    let f = tree.frame_of(floating);
    assert_eq!((f.x, f.y), (0.0, 11.0));
}

/// `top`/`left` on a box with no `position` do nothing, as on a CSS
/// `static` box; `relative` shifts it by them.
#[test]
fn insets_on_a_static_box_are_ignored() {
    let mut tree = LayoutTree::new();
    let root = styled(&mut tree, StyleRules { width: px(800.0), height: px(600.0), ..Default::default() });
    let plain = styled(&mut tree, StyleRules { top: px(9.0), left: px(9.0), height: px(10.0), ..Default::default() });
    let shifted = styled(
        &mut tree,
        StyleRules {
            position: Some(Position::Relative),
            top: px(9.0),
            left: px(9.0),
            height: px(10.0),
            ..Default::default()
        },
    );
    tree.add_child(root, plain);
    tree.add_child(root, shifted);
    tree.compute(root, 800.0, 600.0);
    let f = tree.frame_of(plain);
    assert_eq!((f.x, f.y), (0.0, 0.0));
    let f = tree.frame_of(shifted);
    assert_eq!((f.x, f.y), (9.0, 19.0));
}

/// A wrapper that never had a style applied is `static` too (Taffy's
/// default style), and its absolute child's frame is still re-based into
/// the wrapper's coordinate space.
#[test]
fn never_styled_wrapper_is_static_and_frames_are_parent_relative() {
    let mut tree = LayoutTree::new();
    let root = styled(&mut tree, StyleRules { width: px(800.0), height: px(600.0), ..Default::default() });
    let stage = styled(
        &mut tree,
        StyleRules {
            position: Some(Position::Relative),
            width: pct(100.0),
            height: pct(100.0),
            ..Default::default()
        },
    );
    let seat = styled(&mut tree, StyleRules { width: pct(100.0), height: pct(100.0), ..Default::default() });
    let wrapper = tree.new_node();
    let pin = styled(&mut tree, pin_rules());
    tree.add_child(root, stage);
    tree.add_child(stage, seat);
    tree.add_child(stage, wrapper);
    tree.add_child(wrapper, pin);
    tree.compute(root, 800.0, 600.0);

    assert_eq!(tree.frame_of(wrapper).y, 600.0);
    let p = tree.frame_of(pin);
    assert_eq!((p.x, p.y, p.width, p.height), (0.0, -600.0, 228.0, 600.0));
}

/// REGRESSION: wgpu pins a scroll view's children to `flex_shrink: 0` after
/// their author style is applied. Written as a `set_style` layer it reset the
/// author's `position` to `static`, so the box stopped being the containing
/// block of its absolute children, and a grid's `display`.
#[test]
fn regression_pinning_flex_shrink_keeps_the_authors_position_and_display() {
    let mut tree = LayoutTree::new();
    let root = styled(&mut tree, StyleRules { width: px(800.0), height: px(600.0), ..Default::default() });
    let spacer = styled(&mut tree, StyleRules { height: px(100.0), ..Default::default() });
    let card = styled(
        &mut tree,
        StyleRules { position: Some(Position::Relative), height: px(50.0), ..Default::default() },
    );
    let pin = styled(&mut tree, pin_rules());
    let grid = styled(
        &mut tree,
        StyleRules {
            display: Some(DisplayKind::Grid),
            grid_template_columns: Some(vec![TrackSize::Fr(1.0), TrackSize::Fr(1.0)]),
            width: px(200.0),
            ..Default::default()
        },
    );
    let a = styled(&mut tree, StyleRules { height: px(10.0), ..Default::default() });
    let b = styled(&mut tree, StyleRules { height: px(10.0), ..Default::default() });
    for (parent, child) in [(root, spacer), (root, card), (card, pin), (root, grid), (grid, a), (grid, b)] {
        tree.add_child(parent, child);
    }
    tree.set_flex_shrink(card, 0.0);
    tree.set_flex_shrink(grid, 0.0);
    tree.compute(root, 800.0, 600.0);
    let p = tree.frame_of(pin);
    assert_eq!((p.y, p.height), (0.0, 50.0), "the pin still resolves against the card");
    let second = tree.frame_of(b);
    assert_eq!((second.x, second.y), (100.0, 0.0), "the second cell still sits in the second column");
}
