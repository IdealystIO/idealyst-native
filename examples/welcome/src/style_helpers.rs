//! Short constructors so component files read as property lists,
//! not walls of `Some(Tokenized::Literal(...))`.

use std::rc::Rc;

use framework_core::{Color, Length, StyleRules, StyleSheet, Tokenized};

pub fn static_sheet(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

pub fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

pub fn pct(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Percent(v))
}

pub fn col(s: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(s.into()))
}
