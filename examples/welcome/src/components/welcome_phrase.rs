//! The "Welcome to Idealyst" headline + its centering wrapper.
//!
//! - The wrapper carries the Act 1 entrance (opacity, scale,
//!   translate-y) and the Act 3 shuffle-up that makes room for the
//!   subtitle.
//! - The text inside the wrapper carries the Act 2 dark→light color
//!   transition, written through `drive_color_text_av` on a separate
//!   `TextHandle` ref. (UILabel.textColor is independent of its
//!   parent's tintColor, so color can't ride the wrapper's cascade.)

use std::rc::Rc;

use framework_core::{
    AlignItems, FlexDirection, FontWeight, StyleRules, StyleSheet, TextAlign, Tokenized,
};

use crate::style_helpers::{col, px, static_sheet};
use crate::typeface::INTER;

/// Initial rise distance for the welcome phrase, in CSS pixels.
pub const PHRASE_ENTER_Y: f32 = 24.0;

/// Initial scale of the welcome phrase — slightly under 1.0 so the
/// spring eases up into the resting size with a touch of bounce.
pub const PHRASE_ENTER_SCALE: f32 = 0.95;

/// How far the welcome phrase shuffles UP in Act 3 to make room
/// for the subtitle. Just enough that the subtitle reads as "new
/// information appearing below," not "second line of a paragraph."
pub const WELCOME_SHUFFLE_Y: f32 = -28.0;

pub const HEADLINE_SIZE_PX: f32 = 56.0;

pub const COLOR_HEADLINE_DARK: &str = "#0a0c11";
pub const COLOR_HEADLINE_LIGHT: &str = "#f4ead8";

/// Wrapper around the welcome phrase. Opacity 0 at start (the
/// `welcome_opacity` AV animates it up in Act 1).
pub fn welcome_wrapper_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        opacity: Some(Tokenized::Literal(0.0)),
        // Dark ink at rest; `welcome_color` AV transitions this to
        // the light cream during Act 2. The animated inline color
        // overrides this initial value.
        color: Some(col(COLOR_HEADLINE_DARK)),
        ..Default::default()
    })
}

/// Headline text style — the welcome phrase wears this. The
/// initial `color` value matches `COLOR_HEADLINE_DARK` so the
/// first paint reads correctly on the light frame; once the
/// timeline kicks in, the AV-driven inline color override on the
/// UILabel (iOS) / `style.color` (web) carries the dark→light
/// transition through Act 2.
pub fn headline_sheet() -> Rc<StyleSheet> {
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
