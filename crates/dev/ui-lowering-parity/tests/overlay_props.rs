//! Regression: `overlay(click_through = true)` compiled and was dropped.
//!
//! `emit_overlay` lowered a hand-written list of props by name and
//! `click_through` was not on it, so the prop reached nothing: the portal
//! root stayed interactive and an empty toast strip swallowed the clicks
//! under it — the exact bug `click_through` exists to fix. The builder
//! half has its own test (`click_through_marks_portal_root_pointer_events_none`
//! in runtime-vocabulary); this is the `ui!` half, observed on the built
//! element rather than on the emitted tokens.
//! `runtime-macros`' `setter_tables_cover_every_glue_setter` is what stops
//! the next glue setter from going the same way.

use runtime_macros::ui;
use runtime_scene::Element;
use runtime_shared::primitives::overlay::BackdropMode;
use runtime_shared::PointerEvents;
use runtime_vocabulary::prims::{PortalPrim, PrimCell};
use runtime_vocabulary::style_attach::StyleProp;

fn portal_of(el: &Element) -> PortalPrim {
    match el {
        Element::Item { data, .. } => data
            .downcast_ref::<PrimCell<PortalPrim>>()
            .expect("overlay lowers to a PortalPrim item")
            .take(),
        _ => panic!("overlay lowering must produce an Item"),
    }
}

#[test]
fn regression_ui_overlay_click_through_reaches_the_portal() {
    let el: Element = ui! {
        overlay(backdrop = BackdropMode::None, click_through = true) {
            text { "toast" }
        }
    };
    match portal_of(&el).style {
        Some(StyleProp::Static(rules)) => assert_eq!(
            rules.pointer_events,
            Some(PointerEvents::None),
            "`click_through = true` must mark the portal root pointer-events:none"
        ),
        _ => panic!("`click_through = true` was dropped: the portal root has no style"),
    }
}

#[test]
fn an_overlay_without_click_through_stays_interactive() {
    let el: Element = ui! {
        overlay(backdrop = BackdropMode::None) {
            text { "modal" }
        }
    };
    assert!(portal_of(&el).style.is_none(), "the default portal root stays interactive");
}
