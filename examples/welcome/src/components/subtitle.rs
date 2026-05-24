//! Subtitle under the Act 3 headline. Hidden at start; Act 3 fades
//! it in + slides it up.

use std::rc::Rc;

use runtime_core::{
    component, ui, AlignItems, FlexDirection, FontWeight, Primitive, StyleRules, StyleSheet,
    TextAlign, Tokenized,
};

use crate::coordinator::WelcomeRefs;
use crate::style_helpers::{col, px, static_sheet};
use crate::typeface::INTER;

/// Initial rise (px). Small — primary motion is the fade-in.
pub const SUBTITLE_ENTER_Y: f32 = 10.0;
pub const SUBTITLE_SIZE_PX: f32 = 18.0;
pub const COLOR_SUBTITLE_LIGHT: &str = "#a89a7d";

pub struct SubtitleProps {
    pub refs: WelcomeRefs,
}

#[component]
pub fn subtitle(props: &SubtitleProps) -> Primitive {
    let refs = props.refs;
    let wrapper = wrapper_sheet();
    let text = text_sheet();
    let label = format!(
        "Your {} app starts here.",
        runtime_core::platform().canonical(),
    );
    ui! {
        View(style = wrapper) {
            Text(style = text) { label }
        }.bind(refs.subtitle)
    }
}

fn wrapper_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    })
}

fn text_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_family: Some((&INTER).into()),
        font_size: Some(px(SUBTITLE_SIZE_PX)),
        font_weight: Some(FontWeight::Normal),
        letter_spacing: Some(Tokenized::Literal(0.6)),
        line_height: Some(Tokenized::Literal(SUBTITLE_SIZE_PX + 8.0)),
        text_align: Some(TextAlign::Center),
        color: Some(col(COLOR_SUBTITLE_LIGHT)),
        ..Default::default()
    })
}
