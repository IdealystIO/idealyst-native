//! Short constructors so component files read as property lists,
//! not walls of `Some(Tokenized::Literal(...))`.

use std::rc::Rc;

use runtime_core::{Color, Length, StyleRules, StyleSheet, Tokenized};

/// A named static sheet. The identity feeds `premint_as`, so every
/// welcome style ships as build-time CSS under `--premint` instead of
/// dragging the live style engine in (an ANONYMOUS static sheet has no
/// premint class by construction — measured as 16 fall-throughs on the
/// website's premint report, all from this helper).
///
/// Contract: the id must be unique per rule CONTENT (parameterized
/// sheets bake the parameter into the id, e.g. `planet.2`), and every
/// sheet must be constructed at MOUNT time — both hold here because the
/// welcome scene builds its entire tree at load, which is also when the
/// premint dump's crawl captures it.
pub fn static_sheet(id: &str, rules: StyleRules) -> Rc<StyleSheet> {
    StyleSheet::r#static(rules).premint_as(&format!("welcome.v1.{id}"))
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
