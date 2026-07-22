//! Short constructors so component files read as property lists, not
//! walls of `Some(Tokenized::Literal(...))`.

use std::rc::Rc;

use runtime_core::{
    AlignItems, Color, FlexDirection, JustifyContent, Length, StyleRules, StyleSheet, Tokenized,
};

pub fn static_sheet(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

pub fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

pub fn col(s: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(s.into()))
}

/// A vertical stack with uniform gap and padding.
pub fn column(gap: f32, pad: f32) -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        gap: Some(px(gap)),
        padding_top: Some(px(pad)),
        padding_bottom: Some(px(pad)),
        padding_left: Some(px(pad)),
        padding_right: Some(px(pad)),
        ..Default::default()
    })
}

/// A horizontal row with uniform gap, items vertically centered.
pub fn row(gap: f32) -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::SpaceBetween),
        gap: Some(px(gap)),
        ..Default::default()
    })
}
