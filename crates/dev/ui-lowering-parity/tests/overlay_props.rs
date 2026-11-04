//! Regression: `overlay(click_through = true)` compiled and was dropped.
//!
//! `emit_overlay` lowered a hand-written list of props by name and
//! `click_through` was not on it, so the prop reached nothing and an empty
//! toast strip swallowed the clicks under it — the exact bug `click_through`
//! exists to fix. The builder half has its own test
//! (`click_through_content_inherits_the_root_s_none` in runtime-vocabulary);
//! this is the `ui!` half, observed on the built element rather than on the
//! emitted tokens.
//! `runtime-macros`' `setter_tables_cover_every_glue_setter` is what stops
//! the next glue setter from going the same way.
//!
//! An overlay's root is `pointer-events: none` either way, so only the content
//! wrapper shows whether the prop arrived: `auto` by default, unset (inheriting
//! the root's `none`) under `click_through`.

use runtime_macros::ui;
use runtime_scene::Element;
use runtime_shared::primitives::overlay::BackdropMode;
use runtime_shared::PointerEvents;
use runtime_vocabulary::prims::{PrimCell, ViewPrim};
use runtime_vocabulary::style_attach::StyleProp;

fn wrapper_pointer_events(overlay: &Element) -> Option<PointerEvents> {
    let Element::Item { children, .. } = overlay else {
        panic!("overlay lowering must produce an Item");
    };
    let Element::Item { data, .. } = &children[0] else {
        panic!("the portal's child is the content wrapper");
    };
    let wrapper = data.downcast_ref::<PrimCell<ViewPrim>>().expect("the wrapper is a view").take();
    match wrapper.style {
        Some(StyleProp::Static(rules)) => rules.pointer_events,
        None => None,
        _ => panic!("expected a static wrapper style"),
    }
}

#[test]
fn regression_ui_overlay_click_through_reaches_the_portal() {
    let el: Element = ui! {
        overlay(backdrop = BackdropMode::None, click_through = true) {
            text { "toast" }
        }
    };
    assert_eq!(wrapper_pointer_events(&el), None, "`click_through = true` was dropped: the content takes hits");
}

#[test]
fn an_overlay_without_click_through_stays_interactive() {
    let el: Element = ui! {
        overlay(backdrop = BackdropMode::None) {
            text { "modal" }
        }
    };
    assert_eq!(wrapper_pointer_events(&el), Some(PointerEvents::Auto), "the default content takes hits");
}
