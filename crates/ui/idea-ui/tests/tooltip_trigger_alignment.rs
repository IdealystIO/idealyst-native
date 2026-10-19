//! Regression (reported against idea-ui 8.0.0): a Tooltip moved its
//! trigger. The wrapper it puts around the trigger hugged with
//! `align_self: Center`, which overrides the parent's `align_items` — so
//! in a column the trigger was centred horizontally, and
//! `Stack(align = StackAlign::Start)` could not pull it back. A name with
//! a description tooltip sat off-centre over the left-aligned line below
//! it.
//!
//! A Tooltip adds behaviour, not layout: the trigger must land exactly
//! where it lands without one. Each case lays out the same parent twice —
//! once with the trigger wrapped in a Tooltip, once bare — and compares
//! the trigger's absolute frame. The cases cover both axes and the
//! parent alignments that matter: a column (stretch, start, centre) and a
//! row whose taller sibling stretches the line, where a hugging trigger
//! (Button) must stay vertically centred as it does on its own.
//!
//! The layout is real: mounted through `realize` against `host-mock`,
//! every node's applied `StyleRules` mirrored into
//! `runtime_layout::LayoutTree` (the Taffy wrapper the native backends lay
//! out with), text measured at a fixed advance per char. Same harness as
//! `switch_flex_shrink_layout.rs`. It fails with the wrapper at
//! `align_self: Center`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use idea_ui::{
    install_idea_theme, light_theme, Button, Stack, StackAlign, StackAxis, Tooltip, Typography,
};
use runtime_core::{ui, Element, Length, StyleRules, Tokenized};
use runtime_layout::{AvailableSpace, LayoutNode, LayoutTree, Size};

const PANE_WIDTH: f32 = 600.0;
const PANE_HEIGHT: f32 = 800.0;
const CHAR_ADVANCE: f32 = 8.0;
const LINE_HEIGHT: f32 = 20.0;

const SHORT: &str = "Short name";
const LONG: &str = "a much longer line than the name above it";

/// Single-line text measure: content width at a fixed advance, capped by a
/// definite width.
fn text_measure(content: String) -> runtime_layout::MeasureFn {
    Rc::new(move |known: Size<Option<f32>>, avail: Size<AvailableSpace>| {
        let full = content.chars().count() as f32 * CHAR_ADVANCE;
        let width = match (known.width, avail.width) {
            (Some(w), _) => w,
            (None, AvailableSpace::Definite(w)) => full.min(w),
            (None, _) => full,
        };
        Size { width, height: known.height.unwrap_or(LINE_HEIGHT) }
    })
}

/// Absolute frames (pane coordinates) of every mounted node, by host-mock id.
struct Laid {
    frames: HashMap<u32, runtime_layout::Frame>,
    h: host_mock::Harness,
    root: u32,
}

fn lay_out(build: impl FnOnce() -> Element) -> Laid {
    let h = host_mock::Harness::new();
    let styles: Rc<RefCell<HashMap<u32, StyleRules>>> = Rc::default();
    {
        let styles = styles.clone();
        h.set_style_line(move |n, rules| {
            styles.borrow_mut().insert(n, rules.clone());
            format!("apply_style n{n}")
        });
    }
    let tree = h.world.enter(|| {
        install_idea_theme(light_theme());
        build()
    });
    let _realized = h.mount(tree);
    h.flush();
    let roots = h.live_roots();
    assert_eq!(roots.len(), 1, "one mounted root:\n{}", h.ops().join("\n"));
    let root = roots[0];

    let mut layout = LayoutTree::new();
    let mut ids: HashMap<u32, LayoutNode> = HashMap::new();
    let mut parent: HashMap<u32, u32> = HashMap::new();
    fn mirror(
        h: &host_mock::Harness,
        styles: &HashMap<u32, StyleRules>,
        layout: &mut LayoutTree,
        ids: &mut HashMap<u32, LayoutNode>,
        parent: &mut HashMap<u32, u32>,
        node: u32,
    ) -> LayoutNode {
        let kind = h.kind_of(node).unwrap_or_default();
        // A reactive hole's anchor is layout-transparent on every native
        // backend (`display: contents` on web).
        let ln = if kind == "anchor" { layout.new_contents_node() } else { layout.new_node() };
        // The browser's `min-width: auto` default (see
        // `switch_flex_shrink_layout.rs`, WEB SEMANTICS).
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
            parent.insert(child, node);
            let c = mirror(h, styles, layout, ids, parent, child);
            layout.add_child(ln, c);
        }
        ln
    }
    let root_ln = mirror(&h, &styles.borrow(), &mut layout, &mut ids, &mut parent, root);
    layout.compute(root_ln, PANE_WIDTH, PANE_HEIGHT);

    let local: HashMap<u32, runtime_layout::Frame> =
        ids.iter().map(|(n, ln)| (*n, layout.frame_of(*ln))).collect();
    let frames = local
        .iter()
        .map(|(n, f)| {
            let mut abs = *f;
            let mut cur = *n;
            while let Some(p) = parent.get(&cur) {
                abs.x += local[p].x;
                abs.y += local[p].y;
                cur = *p;
            }
            (*n, abs)
        })
        .collect();
    Laid { frames, h, root }
}

/// The first node (pre-order) whose kind starts with `prefix`.
fn find(h: &host_mock::Harness, node: u32, prefix: &str) -> Option<u32> {
    if h.kind_of(node).is_some_and(|k| k.starts_with(prefix)) {
        return Some(node);
    }
    h.children_of(node).into_iter().find_map(|c| find(h, c, prefix))
}

/// Lay `parent` out with and without a Tooltip around the trigger and
/// assert the node matching `trigger` (a host-mock kind prefix) has the
/// same absolute frame both ways.
fn assert_tooltip_is_transparent(
    case: &str,
    trigger: &str,
    parent: impl Fn(bool) -> Element,
) {
    let bare = lay_out(|| parent(false));
    let wrapped = lay_out(|| parent(true));
    let b = find(&bare.h, bare.root, trigger)
        .unwrap_or_else(|| panic!("{case}: no {trigger} bare:\n{}", bare.h.tree(bare.root)));
    let w = find(&wrapped.h, wrapped.root, trigger)
        .unwrap_or_else(|| panic!("{case}: no {trigger} wrapped:\n{}", wrapped.h.tree(wrapped.root)));
    let (fb, fw) = (bare.frames[&b], wrapped.frames[&w]);
    assert!(
        (fb.x - fw.x).abs() < 0.01
            && (fb.y - fw.y).abs() < 0.01
            && (fb.width - fw.width).abs() < 0.01
            && (fb.height - fw.height).abs() < 0.01,
        "{case}: the Tooltip moved its trigger — bare {fb:?}, wrapped {fw:?}\n{}",
        wrapped.h.tree(wrapped.root)
    );
}

/// A short name trigger over a longer line, in a column with `align`.
fn name_over_line(align: StackAlign, tooltip: bool) -> Element {
    ui! {
        Stack(align = align) {
            if tooltip {
                Tooltip(text = "Describes the name".to_string()) {
                    Typography(content = SHORT.to_string())
                }
            } else {
                Typography(content = SHORT.to_string())
            }
            Typography(content = LONG.to_string())
        }
    }
}

/// The report's exact shape: `Stack(align = Start)`.
#[test]
fn regression_tooltip_centres_its_trigger_in_a_start_aligned_column() {
    assert_tooltip_is_transparent("column, align start", "text \"Short", |t| {
        name_over_line(StackAlign::Start, t)
    });
}

/// The default Stack (stretch) — the trigger was centred here too.
#[test]
fn tooltip_trigger_keeps_its_place_in_a_stretch_column() {
    assert_tooltip_is_transparent("column, align stretch", "text \"Short", |t| {
        name_over_line(StackAlign::Stretch, t)
    });
}

/// A centring parent still centres the trigger.
#[test]
fn tooltip_trigger_keeps_its_place_in_a_centred_column() {
    assert_tooltip_is_transparent("column, align center", "text \"Short", |t| {
        name_over_line(StackAlign::Center, t)
    });
}

/// The case the wrapper's hug exists for: a row stretched by a taller
/// sibling. A Button hugs and centres itself on the cross axis; wrapped,
/// it must not drop to the top of a stretched wrapper.
#[test]
fn tooltip_button_trigger_stays_centred_in_a_stretched_row() {
    assert_tooltip_is_transparent("row, align stretch, Button", "pressable", |tooltip| {
        ui! {
            Stack(axis = StackAxis::Row) {
                if tooltip {
                    Tooltip(text = "Saves the draft".to_string()) {
                        Button(label = "Save".to_string(), on_click = Rc::new(|| {}) as Rc<dyn Fn()>)
                    }
                } else {
                    Button(label = "Save".to_string(), on_click = Rc::new(|| {}) as Rc<dyn Fn()>)
                }
                Stack(gap = idea_ui::StackGap::Md) {
                    Typography(content = LONG.to_string())
                    Typography(content = LONG.to_string())
                    Typography(content = LONG.to_string())
                }
            }
        }
    });
}
