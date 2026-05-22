//! Root frame. Light background, viewport-filling, relative
//! positioning so absolute children can pin to its edges.

use std::rc::Rc;

use framework_core::{Overflow, Position, StyleRules, StyleSheet};

use crate::style_helpers::{col, pct, static_sheet};

pub const COLOR_LIGHT_BG: &str = "#f7f5ef";
pub const COLOR_DARK_BG: &str = "#0a0c11";

pub fn page_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Relative),
        width: Some(pct(100.0)),
        height: Some(pct(100.0)),
        background: Some(col(COLOR_LIGHT_BG)),
        // Clip the sun glare's offscreen overhang to the viewport.
        overflow: Some(Overflow::Hidden),
        ..Default::default()
    })
}
