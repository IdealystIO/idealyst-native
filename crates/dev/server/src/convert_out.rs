//! In-memory → wire conversions used by [`crate::WireRecordingBackend`]
//! when recording scene operations. Symmetric counterpart to the
//! `convert` module in `dev-client`.
//!
//! Style conversions are intentionally one-way + lossy in the same
//! way the in-memory `Tokenized<T>` collapses to its concrete
//! resolved literal when sent on the wire. Tokens are resolved
//! against the dev-side active theme before serialization.

use runtime_shared::accessibility::{AccessibilityProps, LiveRegionPriority, Role};
use runtime_shared::primitives;
use runtime_shared::style::{
    AlignContent, AlignSelf, DisplayKind, FlexWrap, GridPlacement, OverscrollBehavior,
    PointerEvents, ScrollbarVisibility, TextTransform, TrackSize, Transition, UserSelect,
};
use runtime_shared::{
    AlignItems, AssetId, AssetSource, AssetTag, Color, Easing, FlexDirection, FontFamily,
    FontStyle, FontWeight, Gradient, GradientKind, GradientStop, JustifyContent, Length, ObjectFit,
    Overflow, Position, RadialExtent, StateBits, StyleRules, SystemFallback, TextAlign, Tokenized,
    Transform, TypefaceFace, TypefaceId,
};
use wire::{
    AssetId as WireAssetId, TypefaceId as WireTypefaceId, WireAccessibilityAction,
    WireAccessibilityProps, WireAlignItems, WireAssetSource, WireAssetTag, WireColor, WireEasing,
    WireFillRule, WireFlexDirection, WireFontFamily, WireFontStyle, WireFontWeight, WireGradient,
    WireGradientKind, WireGradientStop, WireIconData, WireJustifyContent, WireLength,
    WireLiveRegionPriority, WireObjectFit, WireOverflow, WirePosition, WireRadialExtent, WireRole, WireStateBit,
    WireStyleRules, WireSystemFallback, WireTextAlign, WireTransform, WireTypefaceFace,
};

use crate::HandlerTable;

pub fn icon_data_to_wire(d: &primitives::icon::IconData) -> WireIconData {
    WireIconData {
        view_box: d.view_box,
        paths: d.paths.iter().map(|s| s.to_string()).collect(),
        fill_rule: match d.fill_rule {
            primitives::icon::FillRule::NonZero => WireFillRule::NonZero,
            primitives::icon::FillRule::EvenOdd => WireFillRule::EvenOdd,
        },
        filled: d.filled,
    }
}

/// The `text_input` soft-keyboard hint, as the wire carries it. Reverse:
/// `dev_client::convert::wire_keyboard_kind`.
pub fn keyboard_kind_to_wire(
    k: runtime_shared::primitives::text_input::KeyboardKind,
) -> wire::WireKeyboardKind {
    use runtime_shared::primitives::text_input::KeyboardKind as K;
    match k {
        K::Default => wire::WireKeyboardKind::Default,
        K::Email => wire::WireKeyboardKind::Email,
        K::Phone => wire::WireKeyboardKind::Phone,
        K::Number => wire::WireKeyboardKind::Number,
        K::Decimal => wire::WireKeyboardKind::Decimal,
        K::Url => wire::WireKeyboardKind::Url,
    }
}

pub fn easing_to_wire(e: Easing) -> WireEasing {
    match e {
        Easing::Linear => WireEasing::Linear,
        Easing::Ease => WireEasing::Ease,
        Easing::EaseIn => WireEasing::EaseIn,
        Easing::EaseOut => WireEasing::EaseOut,
        Easing::EaseInOut => WireEasing::EaseInOut,
        Easing::CubicBezier(a, b, c, d) => WireEasing::Cubic(a, b, c, d),
    }
}

/// Bridge `runtime_shared::animation::AnimProp` to its wire mirror.
/// One-to-one map; `GradientStopColor(idx)` carries the same `u8`
/// stop index inline.
pub fn anim_prop_to_wire(p: runtime_shared::animation::AnimProp) -> wire::WireAnimProp {
    use runtime_shared::animation::AnimProp;
    match p {
        AnimProp::Opacity => wire::WireAnimProp::Opacity,
        AnimProp::TranslateX => wire::WireAnimProp::TranslateX,
        AnimProp::TranslateY => wire::WireAnimProp::TranslateY,
        AnimProp::Scale => wire::WireAnimProp::Scale,
        AnimProp::ScaleX => wire::WireAnimProp::ScaleX,
        AnimProp::ScaleY => wire::WireAnimProp::ScaleY,
        AnimProp::RotateZ => wire::WireAnimProp::RotateZ,
        AnimProp::ZIndex => wire::WireAnimProp::ZIndex,
        AnimProp::MaxHeight => wire::WireAnimProp::MaxHeight,
        AnimProp::BackgroundColor => wire::WireAnimProp::BackgroundColor,
        AnimProp::ForegroundColor => wire::WireAnimProp::ForegroundColor,
        AnimProp::GradientStopColor(idx) => wire::WireAnimProp::GradientStopColor(idx),
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
        // The pill crosses as itself: the client resolves it against the
        // box it lays out, as `--local` does. Collapsing it here to
        // `FULL_RADIUS_FALLBACK_PX` drew a 999px-radius rectangle on any
        // box taller than 1998px.
        Length::Full => WireLength::Full,
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

/// `StyleRules` → its wire form. Tokens resolve to their current value
/// (see the module docs); everything else crosses unchanged.
///
/// **Exhaustive by construction.** `StyleRules` is destructured with no
/// `..` and `WireStyleRules` is built with no `..Default::default()`, so
/// a property added to `StyleRules` is a compile error here until it is
/// mapped — and one added to `WireStyleRules` is a compile error until it
/// is filled. `dev_client::convert::wire_style_to_rules` does the same in
/// the other direction. That is what keeps a wire-mode app from silently
/// dropping a property `--local` honours (it once dropped 63 of them);
/// `wire/tests/style_parity.rs` round-trips a value for every field.
pub fn style_rules_to_wire(r: &StyleRules) -> WireStyleRules {
    let StyleRules {
        background,
        color,
        caret_color,
        font_size,
        display,
        grid_template_columns,
        grid_row,
        grid_column,
        flex_direction,
        flex_wrap,
        justify_content,
        align_items,
        align_content,
        gap,
        row_gap,
        column_gap,
        flex_grow,
        flex_shrink,
        flex_basis,
        align_self,
        width,
        height,
        min_width,
        min_height,
        max_width,
        max_height,
        aspect_ratio,
        padding_top,
        padding_right,
        padding_bottom,
        padding_left,
        margin_top,
        margin_right,
        margin_bottom,
        margin_left,
        border_top_left_radius,
        border_top_right_radius,
        border_bottom_left_radius,
        border_bottom_right_radius,
        border_top_width,
        border_right_width,
        border_bottom_width,
        border_left_width,
        border_top_color,
        border_right_color,
        border_bottom_color,
        border_left_color,
        border_style,
        position,
        top,
        right,
        bottom,
        left,
        z_index,
        font_family,
        font_weight,
        font_style,
        line_height,
        letter_spacing,
        text_align,
        underline,
        strikethrough,
        text_transform,
        max_lines,
        opacity,
        overflow,
        overscroll_behavior,
        scrollbar,
        object_fit,
        shadow,
        text_shadow,
        background_gradient,
        transform,
        transform_origin,
        cursor,
        user_select,
        pointer_events,
        background_transition,
        color_transition,
        caret_color_transition,
        opacity_transition,
        transform_transition,
        width_transition,
        height_transition,
        max_width_transition,
        max_height_transition,
        min_width_transition,
        min_height_transition,
        top_transition,
        right_transition,
        bottom_transition,
        left_transition,
        padding_top_transition,
        padding_right_transition,
        padding_bottom_transition,
        padding_left_transition,
        margin_top_transition,
        margin_right_transition,
        margin_bottom_transition,
        margin_left_transition,
        border_top_left_radius_transition,
        border_top_right_radius_transition,
        border_bottom_left_radius_transition,
        border_bottom_right_radius_transition,
        border_top_width_transition,
        border_right_width_transition,
        border_bottom_width_transition,
        border_left_width_transition,
        border_top_color_transition,
        border_right_color_transition,
        border_bottom_color_transition,
        border_left_color_transition,
        shadow_transition,
    } = r;
    let len = |l: &Option<Tokenized<Length>>| l.as_ref().map(tokenized_length);
    let col = |c: &Option<Tokenized<Color>>| c.as_ref().map(tokenized_color);
    let num = |n: &Option<Tokenized<f32>>| n.as_ref().map(tokenized_f32);
    let tr = |t: &Option<Transition>| t.map(transition_to_wire);

    WireStyleRules {
        background: col(background),
        color: col(color),
        font_size: len(font_size),

        flex_direction: flex_direction.map(flex_direction_to_wire),
        justify_content: justify_content.map(justify_content_to_wire),
        align_items: align_items.map(align_items_to_wire),
        gap: len(gap),

        flex_grow: num(flex_grow),
        flex_shrink: num(flex_shrink),
        flex_basis: len(flex_basis),

        width: len(width),
        height: len(height),
        min_width: len(min_width),
        min_height: len(min_height),
        max_width: len(max_width),
        max_height: len(max_height),
        aspect_ratio: *aspect_ratio,

        padding_top: len(padding_top),
        padding_right: len(padding_right),
        padding_bottom: len(padding_bottom),
        padding_left: len(padding_left),
        margin_top: len(margin_top),
        margin_right: len(margin_right),
        margin_bottom: len(margin_bottom),
        margin_left: len(margin_left),

        border_top_left_radius: len(border_top_left_radius),
        border_top_right_radius: len(border_top_right_radius),
        border_bottom_left_radius: len(border_bottom_left_radius),
        border_bottom_right_radius: len(border_bottom_right_radius),

        opacity: num(opacity),
        font_weight: font_weight.map(font_weight_to_wire),
        font_family: font_family.as_ref().map(font_family_to_wire),
        text_align: text_align.map(text_align_to_wire),

        position: position.map(position_to_wire),
        top: len(top),
        right: len(right),
        bottom: len(bottom),
        left: len(left),

        overflow: overflow.map(overflow_to_wire),
        transform: transform.as_ref().map(|t| t.iter().map(transform_to_wire).collect()),
        background_gradient: background_gradient.as_ref().map(gradient_to_wire),
        object_fit: object_fit.map(object_fit_to_wire),

        border_top_width: num(border_top_width),
        border_right_width: num(border_right_width),
        border_bottom_width: num(border_bottom_width),
        border_left_width: num(border_left_width),
        border_top_color: col(border_top_color),
        border_right_color: col(border_right_color),
        border_bottom_color: col(border_bottom_color),
        border_left_color: col(border_left_color),
        shadow: shadow.as_ref().map(shadow_to_wire),
        text_shadow: text_shadow.as_ref().map(shadow_to_wire),
        cursor: cursor.map(cursor_to_wire),
        line_height: num(line_height),
        letter_spacing: num(letter_spacing),
        border_style: border_style.map(border_style_to_wire),
        max_lines: *max_lines,
        z_index: *z_index,

        caret_color: col(caret_color),
        display: display.map(display_to_wire),
        grid_template_columns: grid_template_columns
            .as_ref()
            .map(|t| t.iter().map(track_size_to_wire).collect()),
        grid_row: grid_row.map(grid_placement_to_wire),
        grid_column: grid_column.map(grid_placement_to_wire),
        flex_wrap: flex_wrap.map(flex_wrap_to_wire),
        align_content: align_content.map(align_content_to_wire),
        row_gap: len(row_gap),
        column_gap: len(column_gap),
        align_self: align_self.map(align_self_to_wire),
        font_style: font_style.map(font_style_to_wire),
        underline: *underline,
        strikethrough: *strikethrough,
        text_transform: text_transform.map(text_transform_to_wire),
        overscroll_behavior: overscroll_behavior.map(overscroll_behavior_to_wire),
        scrollbar: scrollbar.map(scrollbar_to_wire),
        transform_origin: transform_origin.map(|(x, y)| (length_to_wire(x), length_to_wire(y))),
        user_select: user_select.map(user_select_to_wire),
        pointer_events: pointer_events.map(pointer_events_to_wire),

        background_transition: tr(background_transition),
        color_transition: tr(color_transition),
        caret_color_transition: tr(caret_color_transition),
        opacity_transition: tr(opacity_transition),
        transform_transition: tr(transform_transition),
        width_transition: tr(width_transition),
        height_transition: tr(height_transition),
        max_width_transition: tr(max_width_transition),
        max_height_transition: tr(max_height_transition),
        min_width_transition: tr(min_width_transition),
        min_height_transition: tr(min_height_transition),
        top_transition: tr(top_transition),
        right_transition: tr(right_transition),
        bottom_transition: tr(bottom_transition),
        left_transition: tr(left_transition),
        padding_top_transition: tr(padding_top_transition),
        padding_right_transition: tr(padding_right_transition),
        padding_bottom_transition: tr(padding_bottom_transition),
        padding_left_transition: tr(padding_left_transition),
        margin_top_transition: tr(margin_top_transition),
        margin_right_transition: tr(margin_right_transition),
        margin_bottom_transition: tr(margin_bottom_transition),
        margin_left_transition: tr(margin_left_transition),
        border_top_left_radius_transition: tr(border_top_left_radius_transition),
        border_top_right_radius_transition: tr(border_top_right_radius_transition),
        border_bottom_left_radius_transition: tr(border_bottom_left_radius_transition),
        border_bottom_right_radius_transition: tr(border_bottom_right_radius_transition),
        border_top_width_transition: tr(border_top_width_transition),
        border_right_width_transition: tr(border_right_width_transition),
        border_bottom_width_transition: tr(border_bottom_width_transition),
        border_left_width_transition: tr(border_left_width_transition),
        border_top_color_transition: tr(border_top_color_transition),
        border_right_color_transition: tr(border_right_color_transition),
        border_bottom_color_transition: tr(border_bottom_color_transition),
        border_left_color_transition: tr(border_left_color_transition),
        shadow_transition: tr(shadow_transition),
    }
}

fn transition_to_wire(t: Transition) -> wire::WireTransition {
    wire::WireTransition { duration_ms: t.duration_ms, easing: easing_to_wire(t.easing) }
}

fn display_to_wire(d: DisplayKind) -> wire::WireDisplay {
    match d {
        DisplayKind::Flex => wire::WireDisplay::Flex,
        DisplayKind::Grid => wire::WireDisplay::Grid,
    }
}

fn track_size_to_wire(t: &TrackSize) -> wire::WireTrackSize {
    use wire::WireTrackSize as W;
    match t {
        TrackSize::Auto => W::Auto,
        TrackSize::MinContent => W::MinContent,
        TrackSize::MaxContent => W::MaxContent,
        TrackSize::Fr(v) => W::Fr(*v),
        TrackSize::Px(v) => W::Px(*v),
        TrackSize::Minmax(lo, hi) => {
            W::Minmax(Box::new(track_size_to_wire(lo)), Box::new(track_size_to_wire(hi)))
        }
        TrackSize::FitContent(v) => W::FitContent(*v),
    }
}

fn grid_placement_to_wire(p: GridPlacement) -> wire::WireGridPlacement {
    match p {
        GridPlacement::Line(l) => wire::WireGridPlacement::Line(l),
        GridPlacement::Lines(a, b) => wire::WireGridPlacement::Lines(a, b),
    }
}

fn flex_wrap_to_wire(w: FlexWrap) -> wire::WireFlexWrap {
    match w {
        FlexWrap::NoWrap => wire::WireFlexWrap::NoWrap,
        FlexWrap::Wrap => wire::WireFlexWrap::Wrap,
        FlexWrap::WrapReverse => wire::WireFlexWrap::WrapReverse,
    }
}

fn align_content_to_wire(a: AlignContent) -> wire::WireAlignContent {
    use wire::WireAlignContent as W;
    match a {
        AlignContent::FlexStart => W::FlexStart,
        AlignContent::FlexEnd => W::FlexEnd,
        AlignContent::Center => W::Center,
        AlignContent::Stretch => W::Stretch,
        AlignContent::SpaceBetween => W::SpaceBetween,
        AlignContent::SpaceAround => W::SpaceAround,
    }
}

fn align_self_to_wire(a: AlignSelf) -> wire::WireAlignSelf {
    use wire::WireAlignSelf as W;
    match a {
        AlignSelf::Auto => W::Auto,
        AlignSelf::FlexStart => W::FlexStart,
        AlignSelf::FlexEnd => W::FlexEnd,
        AlignSelf::Center => W::Center,
        AlignSelf::Stretch => W::Stretch,
        AlignSelf::Baseline => W::Baseline,
    }
}

fn text_transform_to_wire(t: TextTransform) -> wire::WireTextTransform {
    use wire::WireTextTransform as W;
    match t {
        TextTransform::None => W::None,
        TextTransform::Uppercase => W::Uppercase,
        TextTransform::Lowercase => W::Lowercase,
        TextTransform::Capitalize => W::Capitalize,
    }
}

fn overscroll_behavior_to_wire(o: OverscrollBehavior) -> wire::WireOverscrollBehavior {
    use wire::WireOverscrollBehavior as W;
    match o {
        OverscrollBehavior::Auto => W::Auto,
        OverscrollBehavior::Contain => W::Contain,
        OverscrollBehavior::None => W::None,
    }
}

fn scrollbar_to_wire(s: ScrollbarVisibility) -> wire::WireScrollbarVisibility {
    match s {
        ScrollbarVisibility::Auto => wire::WireScrollbarVisibility::Auto,
        ScrollbarVisibility::Hidden => wire::WireScrollbarVisibility::Hidden,
    }
}

fn user_select_to_wire(u: UserSelect) -> wire::WireUserSelect {
    use wire::WireUserSelect as W;
    match u {
        UserSelect::Auto => W::Auto,
        UserSelect::None => W::None,
        UserSelect::Text => W::Text,
        UserSelect::All => W::All,
    }
}

fn pointer_events_to_wire(p: PointerEvents) -> wire::WirePointerEvents {
    match p {
        PointerEvents::Auto => wire::WirePointerEvents::Auto,
        PointerEvents::None => wire::WirePointerEvents::None,
    }
}

/// `BorderStyle` → its wire mirror. Exhaustive, like [`cursor_to_wire`].
fn border_style_to_wire(b: runtime_shared::style::BorderStyle) -> wire::WireBorderStyle {
    use runtime_shared::style::BorderStyle as B;
    use wire::WireBorderStyle as W;
    match b {
        B::Solid => W::Solid,
        B::Dashed => W::Dashed,
        B::Dotted => W::Dotted,
    }
}

pub fn object_fit_to_wire(o: ObjectFit) -> WireObjectFit {
    match o {
        ObjectFit::Fill => WireObjectFit::Fill,
        ObjectFit::Contain => WireObjectFit::Contain,
        ObjectFit::Cover => WireObjectFit::Cover,
    }
}

pub fn position_to_wire(p: Position) -> WirePosition {
    match p {
        Position::Relative => WirePosition::Relative,
        Position::Absolute => WirePosition::Absolute,
        Position::Sticky => WirePosition::Sticky,
    }
}

pub fn overflow_to_wire(o: Overflow) -> WireOverflow {
    match o {
        Overflow::Visible => WireOverflow::Visible,
        Overflow::Hidden => WireOverflow::Hidden,
    }
}

pub fn transform_to_wire(t: &Transform) -> WireTransform {
    match t {
        Transform::TranslateX(l) => WireTransform::TranslateX(length_to_wire(*l)),
        Transform::TranslateY(l) => WireTransform::TranslateY(length_to_wire(*l)),
        Transform::Scale(s) => WireTransform::Scale(*s),
        Transform::ScaleXY { x, y } => WireTransform::ScaleXY { x: *x, y: *y },
        Transform::Rotate(deg) => WireTransform::Rotate(*deg),
        Transform::SkewX(deg) => WireTransform::SkewX(*deg),
        Transform::SkewY(deg) => WireTransform::SkewY(*deg),
    }
}

pub fn gradient_to_wire(g: &Gradient) -> WireGradient {
    WireGradient {
        kind: gradient_kind_to_wire(&g.kind),
        stops: g.stops.iter().map(gradient_stop_to_wire).collect(),
    }
}

pub fn gradient_kind_to_wire(k: &GradientKind) -> WireGradientKind {
    match k {
        GradientKind::Linear { angle_deg } => WireGradientKind::Linear {
            angle_deg: *angle_deg,
        },
        GradientKind::Radial { center, radius, extent } => WireGradientKind::Radial {
            center: *center,
            radius: *radius,
            extent: radial_extent_to_wire(*extent),
        },
    }
}

pub fn gradient_stop_to_wire(s: &GradientStop) -> WireGradientStop {
    WireGradientStop {
        offset: s.offset,
        color: tokenized_color(&s.color),
    }
}

pub fn radial_extent_to_wire(e: RadialExtent) -> WireRadialExtent {
    match e {
        RadialExtent::ClosestSide => WireRadialExtent::ClosestSide,
        RadialExtent::FarthestCorner => WireRadialExtent::FarthestCorner,
    }
}

pub fn font_family_to_wire(ff: &FontFamily) -> WireFontFamily {
    match ff {
        FontFamily::System(name) => WireFontFamily::System(name.clone()),
        FontFamily::Typeface(t) => WireFontFamily::Typeface {
            id: typeface_id_to_wire(t.id),
            family_name: t.family_name.to_string(),
        },
    }
}

pub fn font_style_to_wire(s: FontStyle) -> WireFontStyle {
    match s {
        FontStyle::Normal => WireFontStyle::Normal,
        FontStyle::Italic => WireFontStyle::Italic,
    }
}

pub fn system_fallback_to_wire(f: SystemFallback) -> WireSystemFallback {
    match f {
        SystemFallback::Serif => WireSystemFallback::Serif,
        SystemFallback::SansSerif => WireSystemFallback::SansSerif,
        SystemFallback::Monospace => WireSystemFallback::Monospace,
        SystemFallback::None => WireSystemFallback::None,
    }
}

pub fn asset_id_to_wire(id: AssetId) -> WireAssetId {
    WireAssetId(id.0)
}

pub fn typeface_id_to_wire(id: TypefaceId) -> WireTypefaceId {
    WireTypefaceId(id.0)
}

pub fn asset_tag_to_wire(t: AssetTag) -> WireAssetTag {
    match t {
        AssetTag::Font => WireAssetTag::Font,
        AssetTag::Image => WireAssetTag::Image,
        AssetTag::Audio => WireAssetTag::Audio,
        AssetTag::Video => WireAssetTag::Video,
        AssetTag::Blob => WireAssetTag::Blob,
    }
}

pub fn asset_source_to_wire(s: &AssetSource) -> WireAssetSource {
    match s {
        AssetSource::Embedded { bytes, extension } => WireAssetSource::Embedded {
            bytes: bytes.to_vec(),
            extension: (*extension).to_string(),
        },
        AssetSource::Bundled { path } => WireAssetSource::Bundled {
            path: (*path).to_string(),
        },
        // The AAS client is always the web backend, which links fonts
        // by URL and never needs the bytes — so collapse to the path
        // and keep the (potentially multi-MB) font bytes off the
        // websocket. The client resolves `Bundled` + `Font` to the
        // same `/{path}` served-file URL.
        AssetSource::BundledEmbedded { path, .. } => WireAssetSource::Bundled {
            path: (*path).to_string(),
        },
        AssetSource::Remote { url } => WireAssetSource::Remote {
            url: (*url).to_string(),
        },
    }
}

pub fn typeface_face_to_wire(f: &TypefaceFace) -> WireTypefaceFace {
    WireTypefaceFace {
        weight: font_weight_to_wire(f.weight),
        style: font_style_to_wire(f.style),
        asset: asset_id_to_wire(f.asset),
    }
}

// ---------------------------------------------------------------------------
// Accessibility: runtime_core → wire.
// ---------------------------------------------------------------------------

/// Convert an `&AccessibilityProps` into its wire mirror. Carries
/// label / hint / identifier / hidden / role / traits / live-region
/// across faithfully. For each [`runtime_shared::accessibility::AccessibilityAction`]
/// the recorder allocates a fresh [`wire::HandlerId`] in `handlers`
/// (registering the action's `Rc<dyn Fn()>` so the reverse-channel
/// `AppToDev::Event { handler, args: Unit }` dispatches it). The shape
/// matches how `on_click` / `on_change` cross the wire — see the
/// `WireAccessibilityAction` docs.
pub fn a11y_to_wire(p: &AccessibilityProps, handlers: &mut HandlerTable) -> WireAccessibilityProps {
    WireAccessibilityProps {
        label: p.label.clone(),
        hint: p.hint.clone(),
        identifier: p.identifier.clone(),
        hidden: p.hidden,
        role: p.role.map(role_to_wire),
        traits: p.traits.bits(),
        live_region: p.live_region.map(live_region_to_wire),
        actions: p
            .actions
            .iter()
            .map(|a| WireAccessibilityAction {
                name: a.name.clone(),
                // Mirror the `on_click` pattern: register the closure
                // into the recorder's `HandlerTable` and put the
                // resulting `HandlerId` on the wire. The replayer
                // synthesizes a trampoline that posts
                // `AppToDev::Event { handler: id, args: Unit }` so
                // AT-triggered AX actions on the app side reach the
                // dev-side closure.
                handler: handlers.register_unit(a.handler.clone()),
            })
            .collect(),
    }
}

pub fn role_to_wire(r: Role) -> WireRole {
    match r {
        Role::Button => WireRole::Button,
        Role::Link => WireRole::Link,
        Role::Image => WireRole::Image,
        Role::Text => WireRole::Text,
        Role::Header => WireRole::Header,
        Role::List => WireRole::List,
        Role::ListItem => WireRole::ListItem,
        Role::Group => WireRole::Group,
        Role::Separator => WireRole::Separator,
        Role::TextField => WireRole::TextField,
        Role::TextArea => WireRole::TextArea,
        Role::Switch => WireRole::Switch,
        Role::Slider => WireRole::Slider,
        Role::Checkbox => WireRole::Checkbox,
        Role::RadioButton => WireRole::RadioButton,
        Role::RadioGroup => WireRole::RadioGroup,
        Role::ComboBox => WireRole::ComboBox,
        Role::SearchField => WireRole::SearchField,
        Role::Tab => WireRole::Tab,
        Role::TabList => WireRole::TabList,
        Role::TabPanel => WireRole::TabPanel,
        Role::NavigationLink => WireRole::NavigationLink,
        Role::MenuItem => WireRole::MenuItem,
        Role::Menu => WireRole::Menu,
        Role::MenuBar => WireRole::MenuBar,
        Role::Toolbar => WireRole::Toolbar,
        Role::Alert => WireRole::Alert,
        Role::Status => WireRole::Status,
        Role::ProgressBar => WireRole::ProgressBar,
        Role::Spinner => WireRole::Spinner,
        Role::Dialog => WireRole::Dialog,
        Role::AlertDialog => WireRole::AlertDialog,
        Role::Drawer => WireRole::Drawer,
        Role::Popover => WireRole::Popover,
        Role::Tooltip => WireRole::Tooltip,
        Role::Region => WireRole::Region,
        // `Role` is `#[non_exhaustive]`; future runtime-core variants
        // that this conversion module hasn't been taught about decode
        // as `Unknown` on the receiver. Acceptable because in dev-mode
        // both sides ship from the same commit — the catch-all just
        // keeps us forward-compatible against a future schema bump.
        _ => WireRole::Unknown,
    }
}

pub fn live_region_to_wire(p: LiveRegionPriority) -> WireLiveRegionPriority {
    match p {
        LiveRegionPriority::Polite => WireLiveRegionPriority::Polite,
        LiveRegionPriority::Assertive => WireLiveRegionPriority::Assertive,
    }
}

/// Wire form of a virtualizer layout. The ONE in-memory definition is
/// `runtime_shared::primitives::virtualizer::VirtualLayout` (re-exported
/// as `runtime_shared::VirtualLayout`; both cores share it) —
/// `wire::WireVirtualLayout` is its serde mirror, kept separate only so
/// the `wire` crate stays runtime-free. The matches are exhaustive on
/// purpose: a new `Axis`/`Lanes` variant fails compile HERE (and in
/// `dev-client::convert::wire_virtual_layout`, the inverse) instead of
/// silently dropping on the wire. Round-trip pinned by
/// `mock-backend/tests/wire_virtual_layout_roundtrip.rs`.
pub fn virtual_layout_to_wire(l: runtime_shared::VirtualLayout) -> wire::WireVirtualLayout {
    use primitives::virtualizer::{Axis, Lanes, VirtualLayout};
    let VirtualLayout { axis, lanes, main_spacing, cross_spacing } = l;
    wire::WireVirtualLayout {
        horizontal: match axis {
            Axis::Vertical => false,
            Axis::Horizontal => true,
        },
        lanes: match lanes {
            Lanes::Fixed(n) => wire::WireLanes::Fixed(n),
            Lanes::AutoFit { min_cross } => wire::WireLanes::AutoFit { min_cross },
        },
        main_spacing,
        cross_spacing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `Length` variant reaches the wire, the pill as itself.
    ///
    /// The match in `length_to_wire` has no wildcard so a new `Length`
    /// variant is a build failure here rather than a wrong pixel (this
    /// crate once stopped compiling — and took `idealyst dev` with it —
    /// when `Length::Full` landed everywhere but here).
    ///
    /// `regression_a_pill_radius_crosses_as_a_pill`: the pill used to
    /// collapse to `FULL_RADIUS_FALLBACK_PX`, so a wire-mode client drew
    /// a 999px-radius rectangle where `--local` drew a pill.
    #[test]
    fn regression_a_pill_radius_crosses_as_a_pill() {
        // `matches!` rather than `assert_eq!`: `WireLength` is a
        // serialization type in another crate and derives no `PartialEq`.
        assert!(matches!(length_to_wire(Length::Px(16.0)), WireLength::Px(v) if v == 16.0));
        assert!(matches!(length_to_wire(Length::Percent(50.0)), WireLength::Pct(v) if v == 50.0));
        assert!(matches!(length_to_wire(Length::Auto), WireLength::Auto));
        assert!(matches!(length_to_wire(Length::Full), WireLength::Full));
    }
}

/// `Shadow` → its wire mirror. The colour resolves like every other
/// colour on the wire (see `tokenized_color`).
fn shadow_to_wire(s: &runtime_shared::style::Shadow) -> wire::WireShadow {
    wire::WireShadow {
        x: s.x,
        y: s.y,
        blur: s.blur,
        color: tokenized_color(&s.color),
    }
}

/// `Cursor` → its wire mirror. Exhaustive on purpose: a new cursor
/// variant should be a compile error here, not a silent `Auto`.
fn cursor_to_wire(c: runtime_shared::style::Cursor) -> wire::WireCursor {
    use runtime_shared::style::Cursor as C;
    use wire::WireCursor as W;
    match c {
        C::Auto => W::Auto,
        C::Default => W::Default,
        C::Pointer => W::Pointer,
        C::Text => W::Text,
        C::Wait => W::Wait,
        C::Progress => W::Progress,
        C::Help => W::Help,
        C::NotAllowed => W::NotAllowed,
        C::Move => W::Move,
        C::Grab => W::Grab,
        C::Grabbing => W::Grabbing,
        C::Crosshair => W::Crosshair,
        C::ColResize => W::ColResize,
        C::RowResize => W::RowResize,
        C::EwResize => W::EwResize,
        C::NsResize => W::NsResize,
    }
}

#[cfg(test)]
mod definition_tests {
    use super::*;
    use runtime_shared::style::{Cursor, Shadow};

    /// The recorder half of the v19 definition block. Its twin lives in
    /// `dev-client`'s `convert` suite; together they pin that a card's
    /// edge, a raised surface, a pointer cursor and a line-height
    /// actually leave the sidecar.
    ///
    /// Before this, `style_rules_to_wire` started from the 46-field
    /// subset and silently dropped the rest, so a wire-mode app
    /// rendered flat with no error anywhere.
    #[test]
    fn regression_definition_properties_leave_the_recorder() {
        let mut r = StyleRules::default();
        r.border_top_width = Some(Tokenized::Literal(2.0));
        r.border_left_width = Some(Tokenized::Literal(5.0));
        r.border_top_color = Some(Tokenized::Literal(Color("#102030".to_string())));
        r.shadow = Some(Shadow { x: 0.0, y: 4.0, blur: 12.0, color: Color("#000000".to_string()).into() });
        r.text_shadow = Some(Shadow { x: 1.0, y: 1.0, blur: 2.0, color: Color("#000000".to_string()).into() });
        r.cursor = Some(Cursor::Pointer);
        r.line_height = Some(Tokenized::Literal(1.5));
        r.letter_spacing = Some(Tokenized::Literal(0.4));
        r.border_style = Some(runtime_shared::style::BorderStyle::Dotted);
        r.max_lines = Some(1);
        r.z_index = Some(3);

        let w = style_rules_to_wire(&r);
        assert_eq!(w.max_lines, Some(1));
        assert_eq!(w.z_index, Some(3));
        assert_eq!(w.border_style, Some(wire::WireBorderStyle::Dotted));
        assert_eq!(w.border_top_width, Some(2.0));
        assert_eq!(w.border_left_width, Some(5.0));
        assert!(w.border_top_color.is_some());
        assert!(w.shadow.is_some() && w.text_shadow.is_some());
        assert_eq!(w.cursor, Some(wire::WireCursor::Pointer));
        assert_eq!(w.line_height, Some(1.5));
        assert_eq!(w.letter_spacing, Some(0.4));
    }

    /// A style with none of them set must not start inventing values —
    /// `None` has to stay `None` or every node gains a 0px border.
    #[test]
    fn an_undecorated_style_carries_no_definition() {
        let w = style_rules_to_wire(&StyleRules::default());
        assert!(w.border_top_width.is_none());
        assert!(w.border_top_color.is_none());
        assert!(w.shadow.is_none());
        assert!(w.cursor.is_none());
        assert!(w.line_height.is_none());
        assert!(w.border_style.is_none());
        assert!(w.max_lines.is_none());
        assert!(w.z_index.is_none());
    }
}
