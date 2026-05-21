//! Lower a framework `StyleRules` into a `WireStyle` the BrightScript
//! client consumes.
//!
//! The mapping is intentionally lossy — Roku's SceneGraph has no
//! direct analogue for shadows, transforms, transitions, or per-side
//! borders. We send what the client can express and drop the rest.

use framework_core::{Color, FontWeight, Length, StyleRules, Tokenized};

use crate::command::{
    AlignItems as WireAlignItems, FlexDirection as WireFlexDirection,
    JustifyContent as WireJustifyContent, TextAlign as WireTextAlign, WireColor, WireLength,
    WireStyle,
};

pub fn lower_style(rules: &StyleRules) -> WireStyle {
    WireStyle {
        background: rules.background.as_ref().map(tokenized_color),
        color: rules.color.as_ref().map(tokenized_color),
        font_size: rules.font_size.as_ref().map(|t| length_px(t.value())),
        font_weight: rules.font_weight.map(font_weight_to_numeric),

        width: rules.width.as_ref().map(tokenized_length),
        height: rules.height.as_ref().map(tokenized_length),
        min_width: rules.min_width.as_ref().map(tokenized_length),
        min_height: rules.min_height.as_ref().map(tokenized_length),
        max_width: rules.max_width.as_ref().map(tokenized_length),
        max_height: rules.max_height.as_ref().map(tokenized_length),
        aspect_ratio: rules.aspect_ratio,

        padding_top: rules.padding_top.as_ref().map(|t| length_px(t.value())),
        padding_right: rules.padding_right.as_ref().map(|t| length_px(t.value())),
        padding_bottom: rules.padding_bottom.as_ref().map(|t| length_px(t.value())),
        padding_left: rules.padding_left.as_ref().map(|t| length_px(t.value())),

        margin_top: rules.margin_top.as_ref().map(|t| length_px(t.value())),
        margin_right: rules.margin_right.as_ref().map(|t| length_px(t.value())),
        margin_bottom: rules.margin_bottom.as_ref().map(|t| length_px(t.value())),
        margin_left: rules.margin_left.as_ref().map(|t| length_px(t.value())),

        flex_direction: rules.flex_direction.map(flex_direction),
        justify_content: rules.justify_content.map(justify_content),
        align_items: rules.align_items.map(align_items),
        gap: rules.gap.as_ref().map(|t| length_px(t.value())),

        border_top_left_radius: rules.border_top_left_radius.as_ref().map(|t| length_px(t.value())),
        border_top_right_radius: rules.border_top_right_radius.as_ref().map(|t| length_px(t.value())),
        border_bottom_left_radius: rules.border_bottom_left_radius.as_ref().map(|t| length_px(t.value())),
        border_bottom_right_radius: rules.border_bottom_right_radius.as_ref().map(|t| length_px(t.value())),

        opacity: rules.opacity.as_ref().map(|t| *t.value()),
        text_align: rules.text_align.map(text_align),
    }
}

/// Lower a `Tokenized<Color>` to a `WireColor`. Token references
/// preserve their name + fallback so the BS runtime can re-resolve
/// against the active theme variant; literals pass through.
pub(crate) fn tokenized_color(t: &Tokenized<Color>) -> WireColor {
    match t {
        Tokenized::Literal(c) => WireColor::Literal { value: c.0.clone() },
        Tokenized::Token { name, fallback } => WireColor::Token {
            name: name.to_string(),
            fallback: fallback.0.clone(),
        },
    }
}

/// Lower a `Tokenized<Length>` to a `WireLength`. Same shape as
/// `tokenized_color` — token references carry the name + fallback.
pub(crate) fn tokenized_length(t: &Tokenized<Length>) -> WireLength {
    match t {
        Tokenized::Literal(l) => length(l),
        Tokenized::Token { name, fallback } => WireLength::Token {
            name: name.to_string(),
            fallback: Box::new(length(fallback)),
        },
    }
}

pub(crate) fn length(l: &Length) -> WireLength {
    match l {
        Length::Px(v) => WireLength::Px(*v),
        Length::Percent(v) => WireLength::Percent(*v),
        Length::Auto => WireLength::Auto,
    }
}

/// For properties Roku only expresses in raw pixels (padding, margin,
/// gap, font-size, border-radius). Percent → 0; Auto → 0. Authors who
/// want percentage padding on Roku will need to use width/height
/// percent on the parent instead.
fn length_px(l: &Length) -> f32 {
    match l {
        Length::Px(v) => *v,
        Length::Percent(_) | Length::Auto => 0.0,
    }
}

fn flex_direction(d: framework_core::FlexDirection) -> WireFlexDirection {
    match d {
        framework_core::FlexDirection::Row => WireFlexDirection::Row,
        framework_core::FlexDirection::Column => WireFlexDirection::Column,
        framework_core::FlexDirection::RowReverse => WireFlexDirection::RowReverse,
        framework_core::FlexDirection::ColumnReverse => WireFlexDirection::ColumnReverse,
    }
}

fn justify_content(j: framework_core::JustifyContent) -> WireJustifyContent {
    use framework_core::JustifyContent::*;
    match j {
        FlexStart => WireJustifyContent::Start,
        Center => WireJustifyContent::Center,
        FlexEnd => WireJustifyContent::End,
        SpaceBetween => WireJustifyContent::SpaceBetween,
        SpaceAround => WireJustifyContent::SpaceAround,
        SpaceEvenly => WireJustifyContent::SpaceEvenly,
    }
}

fn align_items(a: framework_core::AlignItems) -> WireAlignItems {
    use framework_core::AlignItems::*;
    match a {
        FlexStart => WireAlignItems::Start,
        Center => WireAlignItems::Center,
        FlexEnd => WireAlignItems::End,
        Stretch => WireAlignItems::Stretch,
        Baseline => WireAlignItems::Baseline,
    }
}

fn text_align(t: framework_core::TextAlign) -> WireTextAlign {
    use framework_core::TextAlign::*;
    match t {
        Left => WireTextAlign::Left,
        Center => WireTextAlign::Center,
        Right => WireTextAlign::Right,
        Justify => WireTextAlign::Justify,
    }
}

/// Numeric weight matches CSS — easiest for the BrightScript client
/// to plug into a font-weight lookup.
fn font_weight_to_numeric(w: FontWeight) -> u32 {
    match w {
        FontWeight::Thin => 100,
        FontWeight::ExtraLight => 200,
        FontWeight::Light => 300,
        FontWeight::Normal => 400,
        FontWeight::Medium => 500,
        FontWeight::SemiBold => 600,
        FontWeight::Bold => 700,
        FontWeight::ExtraBold => 800,
        FontWeight::Black => 900,
    }
}
