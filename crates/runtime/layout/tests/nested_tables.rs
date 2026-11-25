//! Table grids (all-`Auto` columns) are resolved from the tree as it stands:
//! the same answer in every process, on every pass, and whatever the tree was
//! before. A pass pins a table's columns to px, and those pins must not be
//! its content width to the next one.

use std::cell::Cell;
use std::rc::Rc;

use runtime_layout::{AvailableSpace, LayoutNode, LayoutTree, Size};
use runtime_shared::{
    DisplayKind, FlexDirection, Length, Position, StyleRules, Tokenized, TrackSize,
};

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

fn table(cols: usize) -> StyleRules {
    StyleRules {
        display: Some(DisplayKind::Grid),
        grid_template_columns: Some(vec![TrackSize::Auto; cols]),
        ..Default::default()
    }
}

/// Text `w` wide on one line, 10 tall a line, in words 20 wide.
fn text(t: &mut LayoutTree, w: &Rc<Cell<f32>>) -> LayoutNode {
    let n = t.new_node();
    let w = w.clone();
    t.set_measure_fn(
        n,
        Rc::new(move |known: Size<Option<f32>>, space: Size<AvailableSpace>| {
            let w = w.get();
            let max = match space.width {
                AvailableSpace::Definite(x) => x.min(w),
                AvailableSpace::MinContent => w.min(20.0),
                AvailableSpace::MaxContent => w,
            };
            let width = known.width.unwrap_or(max);
            let lines = (w / width.max(1.0)).ceil().max(1.0);
            Size { width, height: known.height.unwrap_or(lines * 10.0) }
        }),
    );
    n
}

fn xw(t: &LayoutTree, n: LayoutNode) -> (f32, f32) {
    let f = t.frame_of(n);
    (f.x, f.width)
}

struct Nested {
    t: LayoutTree,
    root: LayoutNode,
    a: LayoutNode,
    inner: LayoutNode,
    b: LayoutNode,
    c: LayoutNode,
    b_text: Rc<Cell<f32>>,
}

/// A two-column table of `a` and a two-column table of `b` and `c`.
fn nested(a: f32, b: f32, c: f32) -> Nested {
    let mut t = LayoutTree::new();
    let root = t.new_node();
    let outer = t.new_node();
    t.set_author_style(outer, &table(2));
    let inner = t.new_node();
    t.set_author_style(inner, &table(2));
    let b_text = Rc::new(Cell::new(b));
    let a = text(&mut t, &Rc::new(Cell::new(a)));
    let b = text(&mut t, &b_text);
    let c = text(&mut t, &Rc::new(Cell::new(c)));
    t.add_child(root, outer);
    t.add_child(outer, a);
    t.add_child(outer, inner);
    t.add_child(inner, b);
    t.add_child(inner, c);
    Nested { t, root, a, inner, b, c, b_text }
}

/// `(x, width)` of a, the inner table, b, c.
fn columns(n: &Nested) -> [(f32, f32); 4] {
    [n.a, n.inner, n.b, n.c].map(|x| xw(&n.t, x))
}

/// REGRESSION: tables were resolved in `HashMap` order. Inner first, the inner
/// kept columns for a width the outer then did not give it: 400 wide in a 250
/// wide table, in about half of all runs.
#[test]
fn regression_a_nested_table_is_the_same_in_every_tree() {
    for _ in 0..40 {
        let mut n = nested(50.0, 400.0, 30.0);
        n.t.compute(n.root, 300.0, 500.0);
        assert_eq!(columns(&n), [(0.0, 50.0), (50.0, 250.0), (0.0, 220.0), (220.0, 30.0)]);

        let mut n = nested(100.0, 60.0, 40.0);
        n.t.compute(n.root, 400.0, 500.0);
        // 100 and 100 share the spare 200 evenly; so do 60 and 40 of theirs.
        assert_eq!(columns(&n), [(0.0, 200.0), (200.0, 200.0), (0.0, 120.0), (120.0, 80.0)]);
    }
}

/// A second pass gives the first one's answer, and a tree that came to its
/// content and width by steps the answer of one built that way.
#[test]
fn a_nested_table_does_not_depend_on_what_the_tree_was() {
    let mut n = nested(100.0, 60.0, 40.0);
    for (width, b) in [(400.0, 60.0), (400.0, 60.0), (250.0, 60.0), (250.0, 300.0), (600.0, 20.0), (400.0, 60.0)] {
        if n.b_text.replace(b) != b {
            let node = n.b;
            n.t.mark_dirty(node);
        }
        n.t.compute(n.root, width, 500.0);
        let mut fresh = nested(100.0, b, 40.0);
        fresh.t.compute(fresh.root, width, 500.0);
        assert_eq!(columns(&n), columns(&fresh), "{width} wide, b {b}");
    }
}

/// REGRESSION: a table sized by its content never left its first width: its
/// pinned columns were its content. A cell that grew from 50 to 150 got 55.
#[test]
fn regression_a_content_sized_table_follows_its_content() {
    let mut t = LayoutTree::new();
    let root = t.new_node();
    t.set_author_style(
        root,
        &StyleRules { flex_direction: Some(FlexDirection::Row), ..Default::default() },
    );
    let grid = t.new_node();
    t.set_author_style(grid, &table(2));
    let a_text = Rc::new(Cell::new(50.0));
    let a = text(&mut t, &a_text);
    let b = text(&mut t, &Rc::new(Cell::new(60.0)));
    t.add_child(root, grid);
    t.add_child(grid, a);
    t.add_child(grid, b);
    for w in [50.0, 150.0, 30.0] {
        a_text.set(w);
        t.mark_dirty(a);
        t.compute(root, 500.0, 500.0);
        assert_eq!((xw(&t, a), xw(&t, b)), ((0.0, w), (w, 60.0)), "a is {w}");
    }
}

/// A change elsewhere leaves a table alone, and a table in an absolute box
/// inside another's cell follows its containing block, not the outer table.
#[test]
fn an_absolute_table_inside_a_table_follows_its_containing_block() {
    let build = || {
        let mut t = LayoutTree::new();
        let root = t.new_node();
        let outer = t.new_node();
        // A width of its own: the window's is nothing to it.
        t.set_author_style(outer, &StyleRules { width: px(165.0), ..table(1) });
        let cell = t.new_node();
        let inner = t.new_node();
        t.set_author_style(
            inner,
            &StyleRules { position: Some(Position::Absolute), left: px(0.0), ..table(2) },
        );
        let b = text(&mut t, &Rc::new(Cell::new(300.0)));
        let c = text(&mut t, &Rc::new(Cell::new(200.0)));
        t.add_child(root, outer);
        t.add_child(outer, cell);
        t.add_child(cell, inner);
        t.add_child(inner, b);
        t.add_child(inner, c);
        (t, root, [inner, b, c])
    };
    let (mut t, root, nodes) = build();
    for width in [600.0, 250.0, 400.0] {
        t.compute(root, width, 500.0);
        let (mut fresh, fresh_root, fresh_nodes) = build();
        fresh.compute(fresh_root, width, 500.0);
        assert_eq!(nodes.map(|n| xw(&t, n)), fresh_nodes.map(|n| xw(&fresh, n)), "{width} wide");
    }
}

/// REGRESSION: a cell with a width is stretched to its track, and stayed
/// stretched when it was no longer a table's cell (the grid restyled to flex
/// was still resolved as a table), its `max_width` lost for good.
#[test]
fn regression_a_stretched_cell_gets_its_own_width_back() {
    let cell_style = StyleRules { width: px(100.0), max_width: px(120.0), ..Default::default() };
    let build = || {
        let mut t = LayoutTree::new();
        let root = t.new_node();
        let grid = t.new_node();
        t.set_author_style(grid, &table(1));
        let cell = t.new_node();
        t.set_author_style(cell, &cell_style);
        t.add_child(root, grid);
        t.add_child(grid, cell);
        t.compute(root, 400.0, 500.0);
        // The only column is exact and takes the whole 400.
        assert_eq!(xw(&t, cell), (0.0, 400.0));
        (t, root, grid, cell)
    };

    // The grid becomes flex.
    let (mut t, root, grid, cell) = build();
    t.set_author_style(grid, &StyleRules::default());
    t.compute(root, 400.0, 500.0);
    assert_eq!(xw(&t, cell).1, 100.0);

    // The cell moves out.
    let (mut t, root, grid, cell) = build();
    t.remove_child(grid, cell);
    t.add_child(root, cell);
    t.compute(root, 400.0, 500.0);
    assert_eq!(xw(&t, cell).1, 100.0);

    // The cell gives up its width: it fills the column, up to its max.
    let (mut t, root, _, cell) = build();
    t.set_author_style(cell, &StyleRules { width: None, ..cell_style.clone() });
    t.compute(root, 400.0, 500.0);
    assert_eq!(xw(&t, cell).1, 120.0);
}

/// A row measured for its natural height resolves its tables as the layout
/// that follows does.
#[test]
fn measure_main_resolves_tables() {
    let mut t = LayoutTree::new();
    let row = t.new_node();
    let grid = t.new_node();
    t.set_author_style(grid, &table(2));
    // Short hugs its 30; long wraps in the other 70: 3 lines.
    let short = text(&mut t, &Rc::new(Cell::new(30.0)));
    let long = text(&mut t, &Rc::new(Cell::new(200.0)));
    t.add_child(row, grid);
    t.add_child(grid, short);
    t.add_child(grid, long);
    assert_eq!(t.measure_main(row, false, 100.0), 30.0);
    t.compute(row, 100.0, 30.0);
    assert_eq!((xw(&t, short), xw(&t, long)), ((0.0, 30.0), (30.0, 70.0)));
    assert_eq!(t.measure_main(row, false, 400.0), 10.0, "wider: one line");
}
