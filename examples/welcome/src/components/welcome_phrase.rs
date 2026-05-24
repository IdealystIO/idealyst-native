//! "Welcome to Idealyst" headline + its centering wrapper. The
//! wrapper carries the Act 1 entrance and Act 3 shuffle-up; the
//! inner text carries the Act 2 color tween via a separate
//! `TextHandle` ref (UILabel.textColor doesn't cascade from the
//! wrapper's transform).

use std::rc::Rc;

use runtime_core::{
    component, ui, AlignItems, FlexDirection, FontWeight, Primitive, StyleRules, StyleSheet,
    TextAlign, Tokenized,
};

use crate::coordinator::WelcomeRefs;
use crate::style_helpers::{col, px, static_sheet};
use crate::typeface::INTER;

pub const PHRASE_ENTER_Y: f32 = 24.0;

/// Sub-1.0 so the spring eases up into resting size with bounce.
pub const PHRASE_ENTER_SCALE: f32 = 0.95;

/// Act 3 shuffle-up offset (px). Enough that the subtitle reads
/// as a new line, not a paragraph continuation.
pub const WELCOME_SHUFFLE_Y: f32 = -28.0;

pub const HEADLINE_SIZE_PX: f32 = 56.0;
pub const COLOR_HEADLINE_DARK: &str = "#0a0c11";
pub const COLOR_HEADLINE_LIGHT: &str = "#f4ead8";

pub struct WelcomePhraseProps {
    pub refs: WelcomeRefs,
}

#[component]
pub fn welcome_phrase(props: &WelcomePhraseProps) -> Primitive {
    let refs = props.refs;
    let wrapper = wrapper_sheet();
    let headline = headline_sheet();
    ui! {
        View(style = wrapper) {
            Text(style = headline) { "Welcome to Idealyst" }.bind(refs.welcome_text)
        }.bind(refs.welcome)
    }
}

fn wrapper_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        opacity: Some(Tokenized::Literal(0.0)),
        color: Some(col(COLOR_HEADLINE_DARK)),
        ..Default::default()
    })
}

fn headline_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_family: Some((&INTER).into()),
        font_size: Some(px(HEADLINE_SIZE_PX)),
        font_weight: Some(FontWeight::Bold),
        letter_spacing: Some(Tokenized::Literal(-1.6)),
        line_height: Some(Tokenized::Literal(HEADLINE_SIZE_PX + 8.0)),
        text_align: Some(TextAlign::Center),
        color: Some(col(COLOR_HEADLINE_DARK)),
        ..Default::default()
    })
}
