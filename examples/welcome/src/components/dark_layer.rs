//! Full-page dark wash. Fills the entire viewport with the dark
//! color; its opacity is animated 0 → 1 in Act 2.

use std::rc::Rc;

use framework_core::{Position, StyleRules, StyleSheet, Tokenized};

use crate::style_helpers::{col, px, static_sheet};

pub const COLOR_DARK_BG: &str = "#0a0c11";

pub fn dark_layer_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        left: Some(px(0.0)),
        right: Some(px(0.0)),
        bottom: Some(px(0.0)),
        background: Some(col(COLOR_DARK_BG)),
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    })
}
