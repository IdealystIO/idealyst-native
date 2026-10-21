//! Regression: the table pager's "Rows" picker showed a `‹` where its
//! down chevron belongs. The picker sat in an 84px box, but the Select
//! trigger's own `min_width: 160` beat that width, so the trigger spilled
//! 76px past the box and under the page numbers beside it: the
//! pagination's `‹` landed inside the trigger's border and the active page
//! pill covered the chevron.
//!
//! Laid out for real against `host-mock` + `runtime_layout::LayoutTree`
//! with the browser's `min-width: auto` on every node — the harness of
//! `segmented_control_block_layout.rs`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use idea_ui::{install_idea_theme, light_theme, TablePager};
use runtime_core::{ui, Element, Length, StyleRules, Tokenized};
use runtime_layout::{AvailableSpace, LayoutNode, LayoutTree, Size};

/// A table footer's width.
const PANE_WIDTH: f32 = 720.0;
const PANE_HEIGHT: f32 = 800.0;
const CHAR_ADVANCE: f32 = 8.0;
const LINE_HEIGHT: f32 = 20.0;

/// A wrapping text measure: one line at max-content, the longest word at
/// min-content, greedy word wrap at a definite width.
fn text_measure(content: String) -> runtime_layout::MeasureFn {
    Rc::new(move |known: Size<Option<f32>>, avail: Size<AvailableSpace>| {
        let words: Vec<f32> =
            content.split_whitespace().map(|w| w.chars().count() as f32 * CHAR_ADVANCE).collect();
        let space = CHAR_ADVANCE;
        let full: f32 = words.iter().sum::<f32>() + space * (words.len().saturating_sub(1)) as f32;
        let longest = words.iter().copied().fold(0.0, f32::max);
        let width_limit = match (known.width, avail.width) {
            (Some(w), _) => w,
            (None, AvailableSpace::Definite(w)) => w,
            (None, AvailableSpace::MinContent) => longest,
            (None, AvailableSpace::MaxContent) => full,
        };
        let mut lines = 1usize;
        let mut line = 0.0f32;
        let mut widest = 0.0f32;
        for w in &words {
            let next = if line == 0.0 { *w } else { line + space + w };
            if next > width_limit && line > 0.0 {
                widest = widest.max(line);
                lines += 1;
                line = *w;
            } else {
                line = next;
            }
        }
        widest = widest.max(line);
        Size {
            width: known.width.unwrap_or(widest),
            height: known.height.unwrap_or(lines as f32 * LINE_HEIGHT),
        }
    })
}

/// Mount `tree`, mirror it into a `LayoutTree`, lay it out in the pane,
/// and return each mounted node's computed frame (by host-mock node id),
/// plus the node kinds.
fn lay_out(build: impl FnOnce() -> Element) -> (HashMap<u32, runtime_layout::Frame>, host_mock::Harness, u32) {
    let harness = host_mock::Harness::new();
    let styles: Rc<RefCell<HashMap<u32, StyleRules>>> = Rc::default();
    {
        let styles = styles.clone();
        harness.set_style_line(move |n, rules| {
            styles.borrow_mut().insert(n, rules.clone());
            format!("apply_style n{n}")
        });
    }
    let tree = harness.world.enter(|| {
        install_idea_theme(light_theme());
        build()
    });
    let _realized = harness.mount(tree);
    harness.flush();

    let roots = harness.live_roots();
    assert_eq!(roots.len(), 1, "one mounted root:\n{}", harness.ops().join("\n"));
    let root = roots[0];

    let mut layout = LayoutTree::new();
    let mut ids: HashMap<u32, LayoutNode> = HashMap::new();
    fn mirror(
        h: &host_mock::Harness,
        styles: &HashMap<u32, StyleRules>,
        layout: &mut LayoutTree,
        ids: &mut HashMap<u32, LayoutNode>,
        node: u32,
    ) -> LayoutNode {
        let kind = h.kind_of(node).unwrap_or_default();
        // A reactive hole's anchor is layout-transparent on every native
        // backend (`display: contents` on web).
        let ln = if kind == "anchor" { layout.new_contents_node() } else { layout.new_node() };
        // Every node gets the browser's `min-width: auto` unless it sets its
        // own `min_width` (see the module docs: WEB SEMANTICS).
        let mut rules = styles.get(&node).cloned().unwrap_or_default();
        if rules.min_width.is_none() {
            rules.min_width = Some(Tokenized::Literal(Length::Auto));
        }
        layout.set_style(ln, &rules);
        if let Some(content) = kind.strip_prefix("text ") {
            layout.set_measure_fn(ln, text_measure(content.trim_matches('"').to_string()));
        }
        ids.insert(node, ln);
        for child in h.children_of(node) {
            let c = mirror(h, styles, layout, ids, child);
            layout.add_child(ln, c);
        }
        ln
    }
    let root_ln = mirror(&harness, &styles.borrow(), &mut layout, &mut ids, root);
    layout.compute(root_ln, PANE_WIDTH, PANE_HEIGHT);
    let frames = ids.iter().map(|(n, ln)| (*n, layout.frame_of(*ln))).collect();
    (frames, harness, root)
}

/// The first node (pre-order) whose kind starts with `prefix`.
fn find(h: &host_mock::Harness, node: u32, prefix: &str) -> Option<u32> {
    if h.kind_of(node).is_some_and(|k| k.starts_with(prefix)) {
        return Some(node);
    }
    h.children_of(node).into_iter().find_map(|c| find(h, c, prefix))
}

fn parent_of(h: &host_mock::Harness, node: u32, target: u32) -> Option<u32> {
    let kids = h.children_of(node);
    if kids.contains(&target) {
        return Some(node);
    }
    kids.into_iter().find_map(|c| parent_of(h, c, target))
}

fn pressables(h: &host_mock::Harness, node: u32, out: &mut Vec<u32>) {
    if h.kind_of(node).is_some_and(|k| k.starts_with("pressable")) {
        out.push(node);
        return;
    }
    for c in h.children_of(node) {
        pressables(h, c, out);
    }
}


/// Each node's frame in pane coordinates (layout frames are parent-relative).
fn absolute(
    h: &host_mock::Harness,
    frames: &HashMap<u32, runtime_layout::Frame>,
    node: u32,
    origin: (f32, f32),
    out: &mut HashMap<u32, (f32, f32, f32, f32)>,
) {
    let f = frames[&node];
    let (x, y) = (origin.0 + f.x, origin.1 + f.y);
    out.insert(node, (x, y, f.width, f.height));
    for c in h.children_of(node) {
        absolute(h, frames, c, (x, y), out);
    }
}

#[test]
fn regression_table_pager_rows_picker_stays_clear_of_the_page_numbers() {
    let (frames, h, root) = lay_out(|| {
        ui! {
            TablePager(
                offset = 0,
                page_size = 25,
                total = 312,
                noun = "projects".to_string(),
                on_offset = Rc::new(|_| {}) as Rc<dyn Fn(usize)>,
                page_sizes = vec![10, 25, 50, 100],
                on_page_size = Some(Rc::new(|_| {}) as Rc<dyn Fn(usize)>),
            )
        }
    });
    let mut abs = HashMap::new();
    absolute(&h, &frames, root, (0.0, 0.0), &mut abs);
    let mut presses = Vec::new();
    pressables(&h, root, &mut presses);
    // Pre-order: the Rows picker's trigger, then the pagination's `‹`, the
    // page numbers and `›`.
    assert!(presses.len() >= 3, "a trigger and the page buttons:\n{}", h.tree(root));
    let (tx, _, tw, _) = abs[&presses[0]];
    let (px, _, _, _) = abs[&presses[1]];
    assert!(
        tx + tw <= px,
        "the Rows trigger ({tx}..{}) must end before the pagination's first button at {px}:\n{}",
        tx + tw,
        h.tree(root)
    );
    // The chevron (the trigger's last child) sits inside the trigger.
    let chevron = *h.children_of(presses[0]).last().expect("the trigger holds a chevron");
    let (cx, _, cw, _) = abs[&chevron];
    assert!(cx >= tx && cx + cw <= tx + tw, "the chevron sits inside the trigger:\n{}", h.tree(root));
}
