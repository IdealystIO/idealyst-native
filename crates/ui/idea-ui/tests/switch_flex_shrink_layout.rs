//! Regression (reported against idea-ui 3.2.0): a Switch beside a long,
//! wrapping label in a narrow flex row shrank — the track compressed below
//! its declared width while the thumb's slide travel stayed fixed, so the
//! "on" thumb sat off the end of the track.
//!
//! `Stack(axis = Row) { Switch(..) Typography(long text) }` in a 260px
//! pane. The track's declared width is a flex BASIS, and with the default
//! `flex-shrink: 1` a row squeezed by its text shrinks the track down to
//! its content minimum (padding + thumb). The fix pins the track (and the
//! thumb, and the Checkbox box / Radio ring) to `flex_shrink: 0`.
//!
//! This test lays the mounted tree out for real: it mounts through
//! `realize` against `host-mock`, records every node's applied
//! `StyleRules`, mirrors the live node tree into `runtime_layout::LayoutTree`
//! (the Taffy wrapper the native backends lay out with) and reads the
//! track's computed frame. Text nodes get a deterministic wrapping measure
//! (fixed advance per char), standing in for the platform text measurer.
//!
//! WEB SEMANTICS: the bug is a web one. `LayoutTree::set_style` floors a
//! box with a declared width at that width (its own default, so the native
//! backends never shrank the track), while the browser applies CSS
//! `min-width: auto` — for a flex item, the content-based minimum, here
//! padding + thumb. The mirror therefore restores `min-width: auto` on
//! every node that doesn't set a `min_width` of its own, which is exactly
//! the browser's default, so Taffy resolves the squeeze the way the
//! browser does. A real-browser layout test isn't reachable from
//! `cargo test -p idea-ui` (backend-web's browser suite runs under
//! chromedriver and doesn't depend on idea-ui); this is the closest
//! reachable layout. It fails with the track at `flex_shrink: 1`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use idea_theme::extensible::{CHECKBOX_DIMS, RADIO_DIMS, SWITCH_TRACK_DIMS};
use idea_ui::{install_idea_theme, light_theme, Checkbox, Radio, Stack, StackAxis, Switch, Typography};
use runtime_core::{signal, ui, Element, Length, StyleRules, Tokenized};
use runtime_layout::{AvailableSpace, LayoutNode, LayoutTree, Size};

/// The 260px pane from the report.
const PANE_WIDTH: f32 = 260.0;
const PANE_HEIGHT: f32 = 800.0;
/// Deterministic text metrics for the stand-in measurer.
const CHAR_ADVANCE: f32 = 8.0;
const LINE_HEIGHT: f32 = 20.0;

const LONG_LABEL: &str = "Send me a notification every time somebody on my team comments \
     on a document I follow, including replies to my own comments";

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

/// The md entry of a `(size, a, b)` dims table — its first dimension.
fn md(dims: &[(&str, f32, f32)]) -> f32 {
    dims.iter().find(|(k, _, _)| *k == "md").map(|(_, a, _)| *a).unwrap()
}

fn md_track_width() -> f32 {
    md(&SWITCH_TRACK_DIMS)
}

#[test]
fn regression_switch_track_keeps_its_width_beside_wrapping_text() {
    let (frames, h, root) = lay_out(|| {
        let on = signal(true);
        ui! {
            Stack(axis = StackAxis::Row) {
                Switch(value = on, on_change = Rc::new(move |v: bool| on.set(v)) as Rc<dyn Fn(bool)>)
                Typography(content = LONG_LABEL.to_string())
            }
        }
    });
    let track = find(&h, root, "pressable").unwrap_or_else(|| panic!("no track:\n{}", h.tree(root)));
    let track_frame = frames[&track];
    assert_eq!(
        track_frame.width,
        md_track_width(),
        "the Switch track must keep its declared width in a squeezed row \
         (it shrank to {}):\n{}",
        track_frame.width,
        h.tree(root)
    );

    // The thumb, slid fully on, still sits inside the track: its resting
    // x + the md slide travel (16px) + its own width stays within the
    // track's content box (2px inset each side).
    let thumb = h.children_of(track)[0];
    let thumb_frame = frames[&thumb];
    let travel = 16.0;
    assert!(
        thumb_frame.x + travel + thumb_frame.width <= track_frame.width - 2.0 + 0.01,
        "the on-thumb must stay on the track: thumb x={} w={} travel={travel}, track w={}",
        thumb_frame.x,
        thumb_frame.width,
        track_frame.width
    );

    // And the text, not the control, absorbed the squeeze.
    let text = find(&h, root, "text ").unwrap();
    assert!(frames[&text].width < PANE_WIDTH - md_track_width(), "the text wraps to the rest");
}

/// The labelled shape: the track inside the Switch's own label row keeps
/// its width when that row sits in a squeezed parent row too.
#[test]
fn switch_track_keeps_its_width_with_its_own_long_label() {
    let (frames, h, root) = lay_out(|| {
        let on = signal(false);
        ui! {
            Stack(axis = StackAxis::Row) {
                Switch(
                    label = Some(LONG_LABEL.to_string()),
                    value = on,
                    on_change = Rc::new(move |v: bool| on.set(v)) as Rc<dyn Fn(bool)>,
                )
            }
        }
    });
    let track = find(&h, root, "pressable").unwrap();
    assert_eq!(frames[&track].width, md_track_width(), "labelled track width:\n{}", h.tree(root));
}

/// Checkbox and Radio are the sibling fixed-size controls; they shipped the
/// same `flex-shrink: 1` default. Their box / ring must hold its size too.
#[test]
fn checkbox_box_and_radio_ring_keep_their_size_beside_wrapping_text() {
    let (frames, h, root) = lay_out(|| {
        let checked = signal(false);
        ui! {
            Stack(axis = StackAxis::Row) {
                Checkbox(
                    value = checked,
                    on_change = Rc::new(move |v: bool| checked.set(v)) as Rc<dyn Fn(bool)>,
                )
                Typography(content = LONG_LABEL.to_string())
            }
        }
    });
    // The box IS the pressable (the focusable host).
    let f = frames[&find(&h, root, "pressable").unwrap()];
    let side = md(&CHECKBOX_DIMS);
    assert!(
        f.width == side && f.height == side,
        "the Checkbox box keeps its {side}px square under squeeze ({f:?}):\n{}",
        h.tree(root)
    );

    let (frames, h, root) = lay_out(|| {
        let picked = signal(false);
        ui! {
            Stack(axis = StackAxis::Row) {
                Radio(
                    selected = picked,
                    on_select = Rc::new(move || picked.set(true)) as Rc<dyn Fn()>,
                )
                Typography(content = LONG_LABEL.to_string())
            }
        }
    });
    // The ring IS the pressable.
    let f = frames[&find(&h, root, "pressable").unwrap()];
    let side = md(&RADIO_DIMS);
    assert!(
        f.width == side && f.height == side,
        "the Radio ring keeps its {side}px circle under squeeze ({f:?}):\n{}",
        h.tree(root)
    );
}
