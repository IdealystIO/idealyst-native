//! Subtitle under the Act 3 headline. Hidden at start (opacity 0
//! via the wrapper); Act 3 fades it in and slides it up.

use std::rc::Rc;

use framework_core::{
    AlignItems, FlexDirection, FontWeight, StyleRules, StyleSheet, TextAlign, Tokenized,
};

use crate::style_helpers::{col, px, static_sheet};
use crate::typeface::INTER;

/// Initial rise distance for the subtitle. Small — its main motion
/// is the fade-in; the small slide adds character without competing
/// with the welcome's shuffle.
pub const SUBTITLE_ENTER_Y: f32 = 10.0;

pub const SUBTITLE_SIZE_PX: f32 = 18.0;

pub const COLOR_SUBTITLE_LIGHT: &str = "#a89a7d";

/// Wrapper around the subtitle. Opacity 0 at start — Act 3 fades
/// it in and slides it up.
pub fn subtitle_wrapper_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    })
}

pub fn subtitle_sheet() -> Rc<StyleSheet> {
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
