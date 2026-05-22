//! Root frame. Light background, viewport-filling, relative
//! positioning so absolute children can pin to its edges.

use std::rc::Rc;

use framework_core::{Overflow, Position, StyleRules, StyleSheet};

use crate::style_helpers::{col, pct, static_sheet};

pub const COLOR_LIGHT_BG: &str = "#f7f5ef";

pub fn page_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Relative),
        width: Some(pct(100.0)),
        height: Some(pct(100.0)),
        background: Some(col(COLOR_LIGHT_BG)),
        // Clip children that extend past the viewport — the
        // sun-glare anchor is offset negatively so it pokes past
        // the top-right corner, and we want the page edge (not the
        // anchor's bounding box) to be the visible boundary.
        overflow: Some(Overflow::Hidden),
        ..Default::default()
    })
}
