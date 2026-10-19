//! Regression (reported against idea-ui 8.0.0): a menu row couldn't be
//! held to a width. Labels had no line limit, so a long automation name
//! either widened the menu (~830px) or, held narrower, wrapped — and a
//! trailing status mark had nowhere to go.
//!
//! Menu labels are now one line ending in "…". This lays a row out in a
//! box the width of a capped panel (`Menu` / `SubMenu`'s `max_width`) and
//! checks the label shrinks to fit beside the trailing mark instead of
//! pushing it past the edge. Real layout: `tests/common` (host-mock tree
//! mirrored into Taffy). Fails without `max_lines` on `MenuItemLabel`: the
//! label keeps its full width and the mark lands outside the box.

mod common;

use std::rc::Rc;

use common::{lay_out, CHAR_ADVANCE};
use idea_ui::MenuItem;
use runtime_core::{ui, Length, StyleApplication, StyleRules, StyleSheet, Tokenized};

const CAP: f32 = 240.0;
const LONG: &str = "When MG-05 Non-Routine Hazardous Task Authorisation is completed, generate MG-05";

fn capped() -> StyleApplication {
    StyleApplication::new(Rc::new(StyleSheet::r#static(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(CAP))),
        ..Default::default()
    })))
}

#[test]
fn regression_long_menu_label_ellipsizes_beside_its_trailing_mark() {
    let laid = lay_out(800.0, 600.0, || {
        ui! {
            view(style = capped) {
                MenuItem(
                    label = LONG.to_string(),
                    on_select = Rc::new(|| {}) as Rc<dyn Fn()>,
                    trailing = Some(ui! { text { "Active".to_string() } }),
                )
            }
        }
    });
    let label = laid.frame("text \"When MG-05");
    let mark = laid.frame("text \"Active");
    let full = LONG.chars().count() as f32 * CHAR_ADVANCE;

    assert!(
        label.width < full && label.width > 0.0,
        "the label shrinks to the row (full {full}, got {}):\n{}",
        label.width,
        laid.h.tree(laid.root)
    );
    assert!(
        mark.x + mark.width <= CAP + 0.01,
        "the trailing mark stays inside the {CAP}px cap (ends at {}):\n{}",
        mark.x + mark.width,
        laid.h.tree(laid.root)
    );
    assert!(label.x + label.width <= mark.x + 0.01, "the label ends before the mark");
}
