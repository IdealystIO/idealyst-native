//! The viewport-portal placement contract: every native backend lays its
//! portal container out with
//! `runtime_shared::primitives::portal::viewport_container_rules`, and the
//! content child must land where the web backend puts it (web pins the
//! portal root with CSS — `viewport_placement_styles` in `backend-web` — and
//! the content lays out as a block inside it: full root width, natural
//! height).
//!
//! The content here is a wrapper view (no size of its own) around a
//! 100×40 box, so "natural size" is 100×40 and "full width" is the viewport.

use runtime_layout::LayoutTree;
use runtime_shared::primitives::portal::{viewport_container_rules, ViewportPlacement};
use runtime_shared::{Length, PointerEvents, StyleRules};

const VIEW_W: f32 = 400.0;
const VIEW_H: f32 = 300.0;
const CONTENT_W: f32 = 100.0;
const CONTENT_H: f32 = 40.0;

/// Lay out `placement` and return the content wrapper's frame
/// `(x, y, width, height)`. `portal_style` is applied the way a backend's
/// `apply_style` does for a portal container: layered OVER the placement.
fn content_box(placement: ViewportPlacement, portal_style: Option<StyleRules>) -> (f32, f32, f32, f32) {
    let mut tree = LayoutTree::new();
    let container = tree.new_node();
    let rules = viewport_container_rules(placement);
    let rules = match portal_style {
        Some(s) => rules.merge(&s),
        None => rules,
    };
    tree.set_style(container, &rules);

    let wrapper = tree.new_node();
    tree.set_style(wrapper, &StyleRules::default());
    let leaf = tree.new_node();
    tree.set_style(
        leaf,
        &StyleRules {
            width: Some(Length::Px(CONTENT_W).into()),
            height: Some(Length::Px(CONTENT_H).into()),
            ..Default::default()
        },
    );
    tree.add_child(wrapper, leaf);
    tree.add_child(container, wrapper);
    tree.compute(container, VIEW_W, VIEW_H);
    let f = tree.frame_of(wrapper);
    (f.x, f.y, f.width, f.height)
}

/// The web box for each placement (`backend-web` `viewport_placement_styles`
/// + block layout of the content inside the pinned root).
fn web_box(placement: ViewportPlacement) -> (f32, f32, f32, f32) {
    match placement {
        // `top: 0; left: 0; right: 0` — full width, natural height, top.
        ViewportPlacement::Top => (0.0, 0.0, VIEW_W, CONTENT_H),
        // `bottom: 0; left: 0; right: 0` — full width, natural height, bottom.
        ViewportPlacement::Bottom => (0.0, VIEW_H - CONTENT_H, VIEW_W, CONTENT_H),
        // `top: 0; bottom: 0; left: 0` — shrink-to-fit width; the block
        // child keeps its natural height at the root's TOP.
        ViewportPlacement::Left => (0.0, 0.0, CONTENT_W, CONTENT_H),
        // `top: 0; bottom: 0; right: 0` — same, against the right edge.
        ViewportPlacement::Right => (VIEW_W - CONTENT_W, 0.0, CONTENT_W, CONTENT_H),
        // `inset: 0` — full width, natural height, top.
        ViewportPlacement::FullScreen => (0.0, 0.0, VIEW_W, CONTENT_H),
        // centered by translate, natural size.
        ViewportPlacement::Center => (
            (VIEW_W - CONTENT_W) / 2.0,
            (VIEW_H - CONTENT_H) / 2.0,
            CONTENT_W,
            CONTENT_H,
        ),
    }
}

const ALL: [ViewportPlacement; 6] = [
    ViewportPlacement::Top,
    ViewportPlacement::Bottom,
    ViewportPlacement::Left,
    ViewportPlacement::Right,
    ViewportPlacement::FullScreen,
    ViewportPlacement::Center,
];

/// REGRESSION: the native backends each kept their own placement mapping
/// and disagreed with web and each other — Linux centered `Left`/`Right`
/// content vertically and `Top`/`Bottom`/`FullScreen` content horizontally,
/// iOS padded `Center` by 16pt, and Android ignored placement (every
/// overlay laid out like `Top`).
#[test]
fn every_placement_puts_content_where_web_does() {
    for placement in ALL {
        assert_eq!(
            content_box(placement, None),
            web_box(placement),
            "{placement:?}: native content box must match web's",
        );
    }
}

/// REGRESSION (CrewForge kiosk, 2026-10-15): a click-through overlay's
/// portal style (`pointer_events: None` only) must not move the content.
/// Backends layer it over the placement; replacing the placement with it
/// reset `justify_content` and pulled a `Bottom` toast host to the top.
#[test]
fn a_portal_style_layered_over_the_placement_keeps_it() {
    let click_through = StyleRules { pointer_events: Some(PointerEvents::None), ..Default::default() };
    for placement in ALL {
        assert_eq!(
            content_box(placement, Some(click_through.clone())),
            web_box(placement),
            "{placement:?}: the click-through style must not move the content",
        );
    }
}
