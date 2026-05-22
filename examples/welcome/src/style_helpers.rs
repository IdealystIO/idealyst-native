//! Tiny constructors used by the per-component stylesheet builders.
//! They wrap the framework's `StyleSheet::static`, `Length::Px`,
//! `Length::Percent`, and `Color` types in shorter call sites so the
//! component files read as a list of property assignments rather
//! than a wall of `Some(Tokenized::Literal(...))`.

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
