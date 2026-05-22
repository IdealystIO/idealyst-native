//! Absolutely-positioned column that holds the welcome phrase and
//! the subtitle. Flex-centered so the pair sits in the middle of
//! the page; the welcome ends up slightly above center, subtitle
//! slightly below.

use std::rc::Rc;

use framework_core::{AlignItems, FlexDirection, JustifyContent, Position, StyleRules, StyleSheet};

use crate::style_helpers::{px, static_sheet};

pub fn content_layer_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        left: Some(px(0.0)),
        right: Some(px(0.0)),
        bottom: Some(px(0.0)),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        // Modest gap between the headline and where the subtitle
        // lands. The welcome's Act 3 shuffle-up adds visual breathing
        // room on top of this.
        gap: Some(px(14.0)),
        ..Default::default()
    })
}
