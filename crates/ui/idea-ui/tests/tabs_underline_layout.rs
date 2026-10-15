//! The tab bar's geometry, laid out for real: the active tab's underline
//! must COVER the divider, the divider must span the bar's full width, and
//! `padding_horizontal` must inset the tabs without shortening the divider.
//!
//! Same technique as `switch_flex_shrink_layout.rs`: mount `Tabs` through
//! `realize` against `host-mock`, record every node's applied `StyleRules`,
//! mirror the live node tree into `runtime_layout::LayoutTree` (the Taffy
//! wrapper the native backends lay out with) and read computed frames. Text
//! gets a fixed-advance measure standing in for the platform measurer.
//!
//! Overlap is two facts, and both are pinned here:
//! - GEOMETRY: the rule's 1px band lies inside the active tab's bottom 2px
//!   (its underline border) — checked on the laid-out frames;
//! - PAINT ORDER: the rule is the host's FIRST child and the scroller after
//!   it is positioned, so the tabs paint over the rule on native (child
//!   order) and on web (positioned siblings paint in DOM order). Checked on
//!   the mounted tree and the applied styles. A real paint isn't reachable
//!   from `cargo test`; this is the closest reachable check.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use idea_ui::{install_idea_theme, light_theme, StackPadding, Tab, Tabs};
use runtime_core::{signal, ui, Element, Length, Position, StyleRules, Tokenized};
use runtime_layout::{AvailableSpace, Frame, LayoutNode, LayoutTree, Size};

const PANE_WIDTH: f32 = 400.0;
const PANE_HEIGHT: f32 = 300.0;
const CHAR_ADVANCE: f32 = 8.0;
const LINE_HEIGHT: f32 = 20.0;
/// The active tab's underline (`TabButton`'s `border_bottom_width`).
const UNDERLINE: f32 = 2.0;

fn text_measure(content: String) -> runtime_layout::MeasureFn {
    Rc::new(move |known: Size<Option<f32>>, _avail: Size<AvailableSpace>| Size {
        width: known.width.unwrap_or(content.chars().count() as f32 * CHAR_ADVANCE),
        height: known.height.unwrap_or(LINE_HEIGHT),
    })
}

struct Laid {
    /// Keeps the mounted tree live while the test reads it.
    _realized: runtime_scene::Realized<host_mock::Node>,
    h: host_mock::Harness,
    root: u32,
    frames: HashMap<u32, Frame>,
    styles: HashMap<u32, StyleRules>,
    parent: HashMap<u32, u32>,
}

impl Laid {
    /// Window-space frame: Taffy frames are parent-relative.
    fn abs(&self, node: u32) -> Frame {
        let mut f = self.frames[&node];
        let mut at = node;
        while let Some(&p) = self.parent.get(&at) {
            f.x += self.frames[&p].x;
            f.y += self.frames[&p].y;
            at = p;
        }
        f
    }

    fn kind(&self, node: u32) -> String {
        self.h.kind_of(node).unwrap_or_default()
    }

    /// Every node (pre-order) whose kind starts with `prefix`.
    fn all(&self, node: u32, prefix: &str, out: &mut Vec<u32>) {
        if self.kind(node).starts_with(prefix) {
            out.push(node);
        }
        for c in self.h.children_of(node) {
            self.all(c, prefix, out);
        }
    }
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
    let realized = h.mount(tree);
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
    let styles = styles.borrow().clone();
    let root_ln = mirror(&h, &styles, &mut layout, &mut ids, &mut parent, root);
    layout.compute(root_ln, PANE_WIDTH, PANE_HEIGHT);
    let frames = ids.iter().map(|(n, ln)| (*n, layout.frame_of(*ln))).collect();
    Laid { _realized: realized, h, root, frames, styles, parent }
}

fn tabs(padding: StackPadding) -> Laid {
    lay_out(move || {
        let list = signal(vec![Tab::new("a", "Alpha"), Tab::new("b", "Beta"), Tab::new("c", "Gamma")]);
        let active = signal("b".to_string());
        ui! {
            Tabs(
                tabs = list,
                active = active,
                on_change = Rc::new(move |id: String| active.set(id)) as Rc<dyn Fn(String)>,
                padding_horizontal = padding,
            )
        }
    })
}

/// The host is the root; its children are `[rule, scroller]`.
fn host_parts(l: &Laid) -> (u32, u32, u32) {
    let host = l.root;
    let kids = l.h.children_of(host);
    assert_eq!(kids.len(), 2, "host holds the rule and the scroller:\n{}", l.h.tree(host));
    (host, kids[0], kids[1])
}

#[test]
fn the_active_underline_covers_the_divider() {
    let l = tabs(StackPadding::None);
    let (host, rule, scroller) = host_parts(&l);
    let host_f = l.abs(host);
    let rule_f = l.abs(rule);

    // The rule: 1px, full width, on the bar's bottom edge.
    assert_eq!(rule_f.height, 1.0, "the divider is a 1px rule");
    assert_eq!((rule_f.x, rule_f.width), (host_f.x, host_f.width), "the divider spans the bar");
    assert_eq!(rule_f.y + rule_f.height, host_f.y + host_f.height, "the divider sits on the bar's bottom edge");

    // The active tab ("Beta", the second pressable) ends on the same edge,
    // so its 2px underline band contains the rule's 1px band.
    let mut tabs = Vec::new();
    l.all(host, "pressable", &mut tabs);
    assert_eq!(tabs.len(), 3, "{}", l.h.tree(host));
    let tab_f = l.abs(tabs[1]);
    let underline_top = tab_f.y + tab_f.height - UNDERLINE;
    let underline_bottom = tab_f.y + tab_f.height;
    assert!(
        underline_top <= rule_f.y && rule_f.y + rule_f.height <= underline_bottom,
        "the underline [{underline_top}, {underline_bottom}] must cover the divider [{}, {}]",
        rule_f.y,
        rule_f.y + rule_f.height
    );
    assert!(
        rule_f.x <= tab_f.x && tab_f.x + tab_f.width <= rule_f.x + rule_f.width,
        "the divider runs under the whole active tab"
    );

    // Paint order: rule first, scroller positioned after it.
    assert_eq!(
        l.styles[&rule].position,
        Some(Position::Absolute),
        "the rule is out of flow so it takes no height"
    );
    assert_eq!(
        l.styles[&scroller].position,
        Some(Position::Relative),
        "the scroller is positioned, so the web paints it (and the underline) above the rule"
    );
    assert!(
        l.kind(scroller).starts_with("scroll"),
        "the scroller follows the rule: {}",
        l.h.tree(host)
    );

    // The bar no longer draws its own border — a border can't be overlapped.
    let host_border = l.styles[&host].border_bottom_width.as_ref().map(|t| t.resolve()).unwrap_or(0.0);
    assert_eq!(host_border, 0.0, "the divider is the rule, not a border on the bar");
}

#[test]
fn padding_insets_the_tabs_but_not_the_divider() {
    let flush = tabs(StackPadding::None);
    let padded = tabs(StackPadding::Md);

    let first_tab_x = |l: &Laid| {
        let mut tabs = Vec::new();
        l.all(l.root, "pressable", &mut tabs);
        l.abs(tabs[0]).x - l.abs(l.root).x
    };
    let (_, flush_rule, _) = host_parts(&flush);
    let (host, rule, scroller) = host_parts(&padded);
    let strip = padded.h.children_of(scroller)[0];
    let inset = padded.styles[&strip]
        .padding_left
        .as_ref()
        .map(|t| t.resolve())
        .unwrap_or(Length::Px(0.0));
    let Length::Px(inset) = inset else { panic!("md padding resolves to px: {inset:?}") };
    assert!(inset > 0.0, "md padding is a real inset");

    assert_eq!(first_tab_x(&flush), 0.0, "no padding: the first tab sits on the bar's edge");
    assert_eq!(first_tab_x(&padded), inset, "padding insets the first tab");
    assert_eq!(
        (padded.abs(rule).x, padded.abs(rule).width),
        (padded.abs(host).x, padded.abs(host).width),
        "the divider still spans the whole bar"
    );
    assert_eq!(flush.abs(flush_rule).width, padded.abs(rule).width, "padding never shortens the divider");

    // The underline still covers the divider with padding on.
    let mut tabs = Vec::new();
    padded.all(host, "pressable", &mut tabs);
    let tab_f = padded.abs(tabs[1]);
    assert_eq!(tab_f.y + tab_f.height, padded.abs(rule).y + 1.0, "padding is horizontal only");
}
