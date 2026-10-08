//! `presence` is layout-transparent: its child lays out exactly as if it
//! were the direct child of presence's parent.
//!
//! The recording mocks (`host-mock`, `third_party_backend.rs`) see the
//! node tree but not geometry, and the bug this file pins only shows up in
//! geometry. So the host below is a real `runtime_layout::LayoutTree` —
//! the Taffy tree iOS, macOS and Android all drive — with each scene node
//! BEING its layout node: `create_view` mints a plain flex node,
//! `create_anchor` a `display: contents` node, `apply_style` is
//! `set_style`, and the structural ops link the layout tree. What the
//! assertions read is therefore what a native backend would frame its
//! views with.
//!
//! The bug (reported on the iOS 18 iPad simulator):
//!
//! ```ignore
//! // inside a `position: relative` viewfinder
//! presence(|| ui! {
//!     view(style = <position: Absolute; left: md; right: md; bottom: md>) { Alert(..) }
//! })
//! ```
//!
//! never appeared. `PresenceOps::create_presence_placeholder` defaulted to
//! `create_view`, a real flex item. Taffy resolves an absolute child's
//! insets against its DIRECT layout parent (every Taffy node is a
//! containing block), so the alert was positioned against the placeholder
//! — full width, ZERO height (its only child is out of flow) at the top of
//! the viewfinder — and `bottom: md` put it above the viewfinder's top
//! edge, off screen. Web never showed it: a static `<div>` is not a CSS
//! containing block, so the same insets resolved against the viewfinder.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_layout::{LayoutNode, LayoutTree};
use runtime_vocabulary::backend::{
    self as fw, caps, install_env_services, realize, register_builtins_with, AllBuiltins, Element,
    Host, Registry, World,
};
use runtime_vocabulary::builders::{presence, view};
use fw::runtime_shared::accessibility::AccessibilityProps;
use fw::runtime_shared::primitives::icon::IconData;
use fw::runtime_shared::{
    Action, FlexDirection, Length, Position, StyleRules, Tokenized,
};

// ---------------------------------------------------------------------------
// A host whose nodes are layout nodes
// ---------------------------------------------------------------------------

struct LayoutHost {
    layout: LayoutTree,
    /// Every `create_view` node, in creation order (the scene creates a
    /// parent before its children, so author order = index order).
    views: Vec<LayoutNode>,
    splice: bool,
}

impl LayoutHost {
    fn new(splice: bool) -> Self {
        Self { layout: LayoutTree::new(), views: Vec::new(), splice }
    }
}

impl Host for LayoutHost {
    type Node = LayoutNode;

    fn insert(&mut self, parent: &mut LayoutNode, child: LayoutNode) {
        self.layout.add_child(*parent, child);
    }

    fn insert_at(&mut self, parent: &mut LayoutNode, child: LayoutNode, index: usize) {
        self.layout.add_child_at_index(*parent, child, index);
    }

    fn remove_child(&mut self, parent: &LayoutNode, child: &LayoutNode) {
        self.layout.remove_child(*parent, *child);
    }

    fn clear_children(&mut self, node: &LayoutNode) {
        for kid in self.layout.logical_children_of(*node) {
            self.layout.remove_child(*node, kid);
        }
    }

    fn create_anchor(&mut self) -> LayoutNode {
        // What iOS/macOS/Android do: a `display: contents` node.
        self.layout.new_contents_node()
    }

    fn supports_splice(&self) -> bool {
        self.splice
    }
}

impl caps::ViewOps for LayoutHost {
    fn create_view(&mut self, _a11y: &AccessibilityProps) -> LayoutNode {
        let n = self.layout.new_node();
        self.views.push(n);
        n
    }
}

impl caps::StyleOps for LayoutHost {
    fn apply_style(&mut self, node: &LayoutNode, style: &Rc<StyleRules>) {
        self.layout.set_style(*node, style);
    }
}

impl caps::TextOps for LayoutHost {
    fn create_text(&mut self, _content: &str, _a11y: &AccessibilityProps) -> LayoutNode {
        self.layout.new_node()
    }
    fn update_text(&mut self, _node: &LayoutNode, _content: &str) {}
}

impl caps::ButtonOps for LayoutHost {
    fn create_button(
        &mut self,
        _label: &str,
        _on_click: &Action,
        _leading_icon: Option<&IconData>,
        _trailing_icon: Option<&IconData>,
        _a11y: &AccessibilityProps,
    ) -> LayoutNode {
        self.layout.new_node()
    }
}

impl caps::LifecycleOps for LayoutHost {
    fn finish(&mut self, _root: LayoutNode) {}
}

impl caps::AppEnvOps for LayoutHost {
    fn platform(&self) -> fw::Platform {
        fw::Platform::Custom("Layout")
    }
}

// Deliberately NO `PresenceOps` override: the default placeholder is the
// thing under test.
impl caps::PresenceOps for LayoutHost {}
impl caps::InputOps for LayoutHost {}
impl caps::PressableOps for LayoutHost {}
impl caps::AssetOps for LayoutHost {}
impl caps::ExternalOps for LayoutHost {}
impl caps::DocumentOps for LayoutHost {}
impl caps::ImageOps for LayoutHost {}
impl caps::IconOps for LayoutHost {}
impl caps::LinkOps for LayoutHost {}
impl caps::TextInputOps for LayoutHost {}
impl caps::ToggleOps for LayoutHost {}
impl caps::SliderOps for LayoutHost {}
impl caps::ActivityIndicatorOps for LayoutHost {}
impl caps::ScrollOps for LayoutHost {}
impl caps::SafeAreaOps for LayoutHost {}
impl caps::VirtualizerOps for LayoutHost {}
impl caps::GridOps for LayoutHost {}
impl caps::PortalOps for LayoutHost {}
impl caps::NavigatorOps for LayoutHost {}
impl caps::GraphicsOps for LayoutHost {}
impl caps::A11yOps for LayoutHost {}
impl caps::AnimationOps for LayoutHost {}
impl caps::IntrospectionOps for LayoutHost {}
impl caps::BatchOps for LayoutHost {}
impl caps::WireBindingOps for LayoutHost {}

/// Field order is drop order: the realized tree unmounts before the world.
struct Mounted {
    _realized: fw::Realized<LayoutNode>,
    _world: World,
    host: Rc<RefCell<LayoutHost>>,
    root: LayoutNode,
}

fn mount(splice: bool, width: f32, height: f32, build: impl FnOnce() -> Element) -> Mounted {
    let host = Rc::new(RefCell::new(LayoutHost::new(splice)));
    install_env_services(&host);
    let mut registry: Registry<LayoutHost> = Registry::new();
    register_builtins_with::<LayoutHost, AllBuiltins>(&mut registry);
    let registry = Rc::new(registry);
    let world = World::new();
    let realized = world.enter(|| realize(&host, &registry, build()));
    let mut roots = realized.collect_nodes();
    assert_eq!(roots.len(), 1, "single-root mount");
    let root = roots.pop().unwrap();
    host.borrow_mut().layout.compute(root, width, height);
    Mounted { _realized: realized, _world: world, host, root }
}

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

/// The positioned "viewfinder": an explicitly sized, `position: relative`
/// box.
fn viewfinder_style(w: f32, h: f32) -> StyleRules {
    StyleRules {
        position: Some(Position::Relative),
        width: Some(px(w)),
        height: Some(px(h)),
        ..Default::default()
    }
}

/// The reported alert layer: `position: absolute; left/right/bottom: 10`,
/// 20pt tall.
fn bottom_alert_style() -> StyleRules {
    StyleRules {
        position: Some(Position::Absolute),
        left: Some(px(10.0)),
        right: Some(px(10.0)),
        bottom: Some(px(10.0)),
        height: Some(px(20.0)),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The reported bug, on both host shapes: a splice-capable host (iOS,
/// macOS, Android) and a host that nests every reactive region under an
/// anchor (wgpu, SSR-hydration web).
#[test]
fn regression_presence_absolute_child_resolves_against_positioned_ancestor() {
    for splice in [true, false] {
        let m = mount(splice, 300.0, 200.0, || {
            view()
                .style(viewfinder_style(300.0, 200.0))
                .child(presence(|| view().style(bottom_alert_style()).build()))
                .build()
        });
        let h = m.host.borrow();
        assert_eq!(h.views.first(), Some(&m.root), "root is the first view");
        let alert = *h.views.last().unwrap();
        assert_ne!(alert, m.root, "the alert view was created");

        // Same frame as `view(viewfinder) { view(alert) }` with no presence:
        // x = left, width = 300 - left - right, y = 200 - bottom - height.
        let f = h.layout.frame_of(alert);
        assert_eq!(
            (f.x, f.y, f.width, f.height),
            (10.0, 170.0, 280.0, 20.0),
            "splice={splice}: the absolute child must resolve its insets against \
             the viewfinder, not against a presence wrapper box (got {f:?}; the \
             old plain-view placeholder put it at y = -30, above the viewfinder)"
        );
        assert_eq!(
            h.layout.parent_of(alert),
            Some(m.root),
            "splice={splice}: the alert's LAYOUT parent is the viewfinder — presence \
             contributes no box"
        );
    }
}

/// Layout transparency also covers in-flow children: a `flex_grow: 1`
/// child under presence fills its column exactly as a direct child would.
/// A plain-view placeholder hugs the child and collapses it to 0.
#[test]
fn presence_flex_grow_child_fills_the_parent_like_a_direct_child() {
    let m = mount(true, 200.0, 200.0, || {
        view()
            .style(StyleRules {
                flex_direction: Some(FlexDirection::Column),
                width: Some(px(200.0)),
                height: Some(px(200.0)),
                ..Default::default()
            })
            .child(presence(|| {
                view()
                    .style(StyleRules { flex_grow: Some(Tokenized::Literal(1.0)), ..Default::default() })
                    .build()
            }))
            .build()
    });
    let h = m.host.borrow();
    let body = *h.views.last().unwrap();
    assert_eq!(h.layout.frame_of(body).height, 200.0, "flex_grow child fills the column");
}

/// The toast-stack shape the macOS in-flow placeholder fix protected must
/// still hold with a contents placeholder: a column of presence-wrapped
/// cards stacks one below another, keeping the column's `gap`.
#[test]
fn presence_wrapped_cards_still_stack_in_flow_with_gap() {
    let card = || {
        view()
            .style(StyleRules {
                width: Some(px(50.0)),
                height: Some(px(30.0)),
                ..Default::default()
            })
            .build()
    };
    let m = mount(true, 400.0, 400.0, || {
        view()
            .style(StyleRules {
                flex_direction: Some(FlexDirection::Column),
                gap: Some(px(8.0)),
                ..Default::default()
            })
            .child(presence(card))
            .child(presence(card))
            .build()
    });
    let h = m.host.borrow();
    // The cards are the 50×30 views; their y is measured in the column's
    // space (summed up the layout-parent chain) so the assertion holds for
    // any wrapper shape, contents or not.
    let abs_y = |mut n: LayoutNode| {
        let mut y = 0.0;
        while n != m.root {
            y += h.layout.frame_of(n).y;
            n = h.layout.parent_of(n).expect("attached under the root");
        }
        y
    };
    let cards: Vec<f32> = h
        .views
        .iter()
        .filter(|n| {
            let f = h.layout.frame_of(**n);
            (f.width, f.height) == (50.0, 30.0)
        })
        .map(|n| abs_y(*n))
        .collect();
    assert_eq!(cards, [0.0, 38.0], "cards stack one card + gap apart");
}
