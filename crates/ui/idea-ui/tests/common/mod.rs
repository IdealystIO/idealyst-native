//! Real-layout harness for idea-ui integration tests.
//!
//! Mounts a tree through `realize` against `host-mock`, records every
//! node's applied `StyleRules`, mirrors the live node tree into
//! `runtime_layout::LayoutTree` (the Taffy wrapper the native backends lay
//! out with) and computes it in a pane. Text gets a deterministic
//! single-line measure (fixed advance per char), standing in for the
//! platform text measurer.
//!
//! Every node gets the browser's `min-width: auto` unless it sets its own
//! `min_width` — see `switch_flex_shrink_layout.rs` (WEB SEMANTICS) for why
//! that is the faithful default.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use idea_ui::{install_idea_theme, light_theme};
use runtime_core::{Element, Length, StyleRules, Tokenized};
use runtime_layout::{AvailableSpace, Frame, LayoutNode, LayoutTree, Size};

pub const CHAR_ADVANCE: f32 = 8.0;
pub const LINE_HEIGHT: f32 = 20.0;

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

/// A laid-out tree: ABSOLUTE frames (pane coordinates) by host-mock id.
pub struct Laid {
    pub frames: HashMap<u32, Frame>,
    pub h: host_mock::Harness,
    pub root: u32,
}

impl Laid {
    /// The first node (pre-order) whose kind starts with `prefix`.
    pub fn find(&self, prefix: &str) -> Option<u32> {
        find(&self.h, self.root, prefix)
    }

    /// Frame of the first node whose kind starts with `prefix`; panics
    /// with the tree when there is none.
    pub fn frame(&self, prefix: &str) -> Frame {
        let n = self
            .find(prefix)
            .unwrap_or_else(|| panic!("no `{prefix}` node:\n{}", self.h.tree(self.root)));
        self.frames[&n]
    }
}

pub fn find(h: &host_mock::Harness, node: u32, prefix: &str) -> Option<u32> {
    if h.kind_of(node).is_some_and(|k| k.starts_with(prefix)) {
        return Some(node);
    }
    h.children_of(node).into_iter().find_map(|c| find(h, c, prefix))
}

/// Mount `build`'s tree (idea's light theme installed) and lay it out in a
/// `pane_width` × `pane_height` pane.
pub fn lay_out(pane_width: f32, pane_height: f32, build: impl FnOnce() -> Element) -> Laid {
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
    layout.compute(root_ln, pane_width, pane_height);

    let local: HashMap<u32, Frame> = ids.iter().map(|(n, ln)| (*n, layout.frame_of(*ln))).collect();
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
