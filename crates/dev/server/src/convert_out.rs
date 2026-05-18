//! In-memory → wire conversions used by [`crate::WireRecordingBackend`]
//! when recording walker calls. Symmetric counterpart to the
//! `convert` module in `dev-client`.
//!
//! Style conversions are intentionally one-way + lossy in the same
//! way the in-memory `Tokenized<T>` collapses to its concrete
//! resolved literal when sent on the wire. Tokens are resolved
//! against the dev-side active theme before serialization.

use framework_core::primitives;
use framework_core::{
    AlignItems, Color, Easing, FlexDirection, FontWeight, JustifyContent, Length, StateBits,
    StyleRules, TextAlign, Tokenized,
};
use wire::{
    WireAlignItems, WireColor, WireEasing, WireFillRule, WireFlexDirection, WireFontWeight,
    WireIconData, WireJustifyContent, WireLength, WireStateBit, WireStyleRules, WireTextAlign,
};

pub fn icon_data_to_wire(d: &primitives::icon::IconData) -> WireIconData {
    WireIconData {
        view_box: d.view_box,
        paths: d.paths.iter().map(|s| s.to_string()).collect(),
        fill_rule: match d.fill_rule {
            primitives::icon::FillRule::NonZero => WireFillRule::NonZero,
            primitives::icon::FillRule::EvenOdd => WireFillRule::EvenOdd,
        },
    }
}

pub fn easing_to_wire(e: Easing) -> WireEasing {
    match e {
        Easing::Linear => WireEasing::Linear,
        // CSS-default Ease collapses to EaseInOut on the wire — same
        // perceptual category, simplifies the wire enum.
        Easing::Ease => WireEasing::EaseInOut,
        Easing::EaseIn => WireEasing::EaseIn,
        Easing::EaseOut => WireEasing::EaseOut,
        Easing::EaseInOut => WireEasing::EaseInOut,
        Easing::CubicBezier(a, b, c, d) => WireEasing::Cubic(a, b, c, d),
    }
}

pub fn wire_state_bit_to_bits(b: WireStateBit) -> StateBits {
    match b {
        WireStateBit::Hovered => StateBits::HOVERED,
        WireStateBit::Pressed => StateBits::PRESSED,
        WireStateBit::Focused => StateBits::FOCUSED,
        WireStateBit::Disabled => StateBits::DISABLED,
    }
}

/// Expand a `StateBits` bitmask into a list of wire bits. Most
/// overlays carry a single bit, but the framework supports composite
/// bits (`HOVERED | FOCUSED`) by passing them all in one
/// `apply_styled_states` call.
pub fn expand_state_bits(bits: StateBits) -> Vec<WireStateBit> {
    let mut out = Vec::new();
    if bits.contains(StateBits::HOVERED) {
        out.push(WireStateBit::Hovered);
    }
    if bits.contains(StateBits::PRESSED) {
        out.push(WireStateBit::Pressed);
    }
    if bits.contains(StateBits::FOCUSED) {
        out.push(WireStateBit::Focused);
    }
    if bits.contains(StateBits::DISABLED) {
        out.push(WireStateBit::Disabled);
    }
    out
}

fn color_to_wire(c: &Color) -> WireColor {
    WireColor(c.0.clone())
}

fn length_to_wire(l: Length) -> WireLength {
    match l {
        Length::Px(v) => WireLength::Px(v),
        Length::Percent(v) => WireLength::Pct(v),
        Length::Auto => WireLength::Auto,
    }
}

fn flex_direction_to_wire(d: FlexDirection) -> WireFlexDirection {
    match d {
        FlexDirection::Row => WireFlexDirection::Row,
        FlexDirection::Column => WireFlexDirection::Column,
        FlexDirection::RowReverse => WireFlexDirection::RowReverse,
        FlexDirection::ColumnReverse => WireFlexDirection::ColumnReverse,
    }
}

fn justify_content_to_wire(j: JustifyContent) -> WireJustifyContent {
    match j {
        JustifyContent::FlexStart => WireJustifyContent::FlexStart,
        JustifyContent::FlexEnd => WireJustifyContent::FlexEnd,
        JustifyContent::Center => WireJustifyContent::Center,
        JustifyContent::SpaceBetween => WireJustifyContent::SpaceBetween,
        JustifyContent::SpaceAround => WireJustifyContent::SpaceAround,
        JustifyContent::SpaceEvenly => WireJustifyContent::SpaceEvenly,
    }
}

fn align_items_to_wire(a: AlignItems) -> WireAlignItems {
    match a {
        AlignItems::FlexStart => WireAlignItems::FlexStart,
        AlignItems::FlexEnd => WireAlignItems::FlexEnd,
        AlignItems::Center => WireAlignItems::Center,
        AlignItems::Stretch => WireAlignItems::Stretch,
        AlignItems::Baseline => WireAlignItems::Baseline,
    }
}

fn font_weight_to_wire(w: FontWeight) -> WireFontWeight {
    match w {
        FontWeight::Thin => WireFontWeight::Thin,
        FontWeight::ExtraLight => WireFontWeight::ExtraLight,
        FontWeight::Light => WireFontWeight::Light,
        FontWeight::Normal => WireFontWeight::Regular,
        FontWeight::Medium => WireFontWeight::Medium,
        FontWeight::SemiBold => WireFontWeight::SemiBold,
        FontWeight::Bold => WireFontWeight::Bold,
        FontWeight::ExtraBold => WireFontWeight::ExtraBold,
        FontWeight::Black => WireFontWeight::Black,
    }
}

fn text_align_to_wire(a: TextAlign) -> WireTextAlign {
    match a {
        TextAlign::Left => WireTextAlign::Left,
        TextAlign::Right => WireTextAlign::Right,
        TextAlign::Center => WireTextAlign::Center,
        TextAlign::Justify => WireTextAlign::Justify,
    }
}

fn tokenized_color(t: &Tokenized<Color>) -> WireColor {
    color_to_wire(t.value())
}

fn tokenized_length(t: &Tokenized<Length>) -> WireLength {
    length_to_wire(*t.value())
}

fn tokenized_f32(t: &Tokenized<f32>) -> f32 {
    *t.value()
}

pub fn style_rules_to_wire(r: &StyleRules) -> WireStyleRules {
    WireStyleRules {
        background: r.background.as_ref().map(tokenized_color),
        color: r.color.as_ref().map(tokenized_color),
        font_size: r.font_size.as_ref().map(tokenized_length),

        flex_direction: r.flex_direction.map(flex_direction_to_wire),
        justify_content: r.justify_content.map(justify_content_to_wire),
        align_items: r.align_items.map(align_items_to_wire),
        gap: r.gap.as_ref().map(tokenized_length),

        flex_grow: r.flex_grow.as_ref().map(tokenized_f32),
        flex_shrink: r.flex_shrink.as_ref().map(tokenized_f32),
        flex_basis: r.flex_basis.as_ref().map(tokenized_length),

        width: r.width.as_ref().map(tokenized_length),
        height: r.height.as_ref().map(tokenized_length),
        min_width: r.min_width.as_ref().map(tokenized_length),
        min_height: r.min_height.as_ref().map(tokenized_length),
        max_width: r.max_width.as_ref().map(tokenized_length),
        max_height: r.max_height.as_ref().map(tokenized_length),

        padding_top: r.padding_top.as_ref().map(tokenized_length),
        padding_right: r.padding_right.as_ref().map(tokenized_length),
        padding_bottom: r.padding_bottom.as_ref().map(tokenized_length),
        padding_left: r.padding_left.as_ref().map(tokenized_length),
        margin_top: r.margin_top.as_ref().map(tokenized_length),
        margin_right: r.margin_right.as_ref().map(tokenized_length),
        margin_bottom: r.margin_bottom.as_ref().map(tokenized_length),
        margin_left: r.margin_left.as_ref().map(tokenized_length),

        border_top_left_radius: r.border_top_left_radius.as_ref().map(tokenized_length),
        border_top_right_radius: r.border_top_right_radius.as_ref().map(tokenized_length),
        border_bottom_left_radius: r.border_bottom_left_radius.as_ref().map(tokenized_length),
        border_bottom_right_radius: r.border_bottom_right_radius.as_ref().map(tokenized_length),

        opacity: r.opacity.as_ref().map(tokenized_f32),
        font_weight: r.font_weight.map(font_weight_to_wire),
        text_align: r.text_align.map(text_align_to_wire),
    }
}
