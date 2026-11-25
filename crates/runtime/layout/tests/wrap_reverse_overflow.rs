//! A `wrap-reverse` container's lines start at its far cross edge, so lines
//! that don't fit overflow the near one. `align-content` unset is CSS's
//! `stretch`, whose fallback when lines overflow is `flex-start`.

use runtime_layout::LayoutTree;
use runtime_shared::{FlexDirection, FlexWrap, Length, StyleRules, Tokenized};

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

/// Two 80pt boxes, one a line, in a 100pt box.
fn lay_out(direction: FlexDirection, wrap: FlexWrap) -> [(f32, f32); 2] {
    let mut t = LayoutTree::new();
    let root = t.new_node();
    t.set_author_style(
        root,
        &StyleRules {
            width: px(100.0),
            height: px(100.0),
            flex_direction: Some(direction),
            flex_wrap: Some(wrap),
            ..Default::default()
        },
    );
    let items = [0, 1].map(|_| {
        let n = t.new_node();
        t.set_author_style(n, &StyleRules { width: px(80.0), height: px(80.0), ..Default::default() });
        t.add_child(root, n);
        n
    });
    t.compute(root, 100.0, 100.0);
    items.map(|n| {
        let f = t.frame_of(n);
        (f.x, f.y)
    })
}

#[test]
fn regression_wrap_reverse_lines_overflow_the_near_edge() {
    assert_eq!(lay_out(FlexDirection::Row, FlexWrap::WrapReverse), [(0.0, 20.0), (0.0, -60.0)]);
    assert_eq!(lay_out(FlexDirection::Column, FlexWrap::WrapReverse), [(20.0, 0.0), (-60.0, 0.0)]);
    assert_eq!(lay_out(FlexDirection::Row, FlexWrap::Wrap), [(0.0, 0.0), (0.0, 80.0)]);
}
