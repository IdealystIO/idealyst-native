//! Regression (reported against idea-ui 5.2.0): a SegmentedControl could not
//! fill its container. The track pins `align_self: FlexStart` +
//! `flex_shrink: 0` so it hugs its options, and there was no prop to opt
//! out — in a 240px side panel the Align / Vertical controls sat at ~160px
//! while the Fields beside them filled the panel. `block = true` now
//! stretches the track and splits it evenly between the segments.
//!
//! Laid out for real against `host-mock` + `runtime_layout::LayoutTree` (the
//! Taffy wrapper the native backends use), with the browser's
//! `min-width: auto` restored on every node — see
//! `switch_flex_shrink_layout.rs` for why that is the closest reachable
//! stand-in for a browser layout.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use idea_ui::{install_idea_theme, light_theme, ControlSize, SegmentOption, SegmentedControl, Stack};
use runtime_core::{signal, ui, Element, Length, StyleRules, Tokenized};
use runtime_layout::{AvailableSpace, LayoutNode, LayoutTree, Size};

/// The side panel's content width from the report.
const PANE_WIDTH: f32 = 240.0;
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

fn align_control(block: bool) -> Element {
    let v = signal("a".to_string());
    ui! {
        Stack {
            SegmentedControl(
                value = v,
                options = vec![
                    SegmentOption::new("a", "Left"),
                    SegmentOption::new("b", "Centre"),
                    SegmentOption::new("c", "Right"),
                ],
                on_change = Rc::new(move |id: String| v.set(id)) as Rc<dyn Fn(String)>,
                size = ControlSize::Sm,
                block = block,
            )
        }
    }
}

#[test]
fn regression_segmented_control_cannot_fill_its_container() {
    let (frames, h, root) = lay_out(|| align_control(true));
    let mut segs = Vec::new();
    pressables(&h, root, &mut segs);
    assert_eq!(segs.len(), 3, "three segments:\n{}", h.tree(root));
    let track = parent_of(&h, root, segs[0]).expect("segments sit in the track");
    assert_eq!(frames[&track].width, PANE_WIDTH, "block track fills the panel:\n{}", h.tree(root));

    let widths: Vec<f32> = segs.iter().map(|s| frames[s].width).collect();
    for w in &widths {
        // Taffy rounds frames to whole pixels, so an uneven split differs by 1.
        assert!((w - widths[0]).abs() <= 1.0, "segments share the track evenly: {widths:?}");
    }
    let last = frames[segs.last().unwrap()];
    assert!(
        last.x + last.width > PANE_WIDTH - 10.0,
        "the last segment reaches the track's right edge (inside its inset): {last:?}"
    );
}

#[test]
fn segmented_control_hugs_its_options_by_default() {
    let (frames, h, root) = lay_out(|| align_control(false));
    let mut segs = Vec::new();
    pressables(&h, root, &mut segs);
    let track = parent_of(&h, root, segs[0]).expect("segments sit in the track");
    assert!(
        frames[&track].width < PANE_WIDTH,
        "without block the track stays at its content width ({}):\n{}",
        frames[&track].width,
        h.tree(root)
    );
}
