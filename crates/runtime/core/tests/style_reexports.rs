//! Every type a `StyleRules` field holds must be nameable through
//! `runtime_core` — the only style crate an app depends on.
//!
//! Regression (FRAMEWORK-NOTES #71 / Wave-35): `OverscrollBehavior` and
//! `GridPlacement` existed in `runtime-shared` and backed the
//! `overscroll_behavior` / `grid_row` / `grid_column` fields, but were
//! missing from `runtime_vocabulary::glue`'s style re-export list, so an
//! app could not write `StyleRules { grid_column: Some(GridPlacement::SPAN_ALL), .. }`
//! without adding a direct `runtime-shared` dependency.
//!
//! The destructure below is EXHAUSTIVE on purpose: a new `StyleRules`
//! field fails this file until its line is added, and that line only
//! compiles once the field's type is re-exported. Imports come from
//! `runtime_core` alone — never `runtime_shared`.

#![allow(clippy::too_many_lines)]

use runtime_core::{
    AlignContent, AlignItems, AlignSelf, BorderStyle, Color, Cursor, DisplayKind, Easing, FlexDirection,
    FlexWrap, FontFamily, FontStyle, FontWeight, Gradient, GradientKind, GradientStop,
    GridPlacement, JustifyContent, Length, ObjectFit, Overflow, OverscrollBehavior,
    PointerEvents, Position, RadialExtent, ScrollbarVisibility, Shadow, StyleRules, TextAlign,
    TextTransform, Tokenized, TrackSize, Transform, Transition, UserSelect,
};

/// Compile-time: every field's type spelled through `runtime_core`.
#[allow(dead_code, unused_variables)]
fn every_style_rules_field_type_is_nameable(rules: StyleRules) {
    let StyleRules { background, color, caret_color, font_size, display, grid_template_columns, grid_row, grid_column, flex_direction, flex_wrap, justify_content, align_items, align_content, gap, row_gap, column_gap, flex_grow, flex_shrink, flex_basis, align_self, width, height, min_width, min_height, max_width, max_height, aspect_ratio, padding_top, padding_right, padding_bottom, padding_left, margin_top, margin_right, margin_bottom, margin_left, border_top_left_radius, border_top_right_radius, border_bottom_left_radius, border_bottom_right_radius, border_top_width, border_right_width, border_bottom_width, border_left_width, border_top_color, border_right_color, border_bottom_color, border_left_color, border_style, position, top, right, bottom, left, font_family, font_weight, font_style, line_height, letter_spacing, text_align, underline, strikethrough, text_transform, opacity, overflow, overscroll_behavior, scrollbar, object_fit, shadow, text_shadow, background_gradient, transform, transform_origin, cursor, user_select, pointer_events, background_transition, color_transition, caret_color_transition, opacity_transition, transform_transition, width_transition, height_transition, max_width_transition, max_height_transition, min_width_transition, min_height_transition, top_transition, right_transition, bottom_transition, left_transition, padding_top_transition, padding_right_transition, padding_bottom_transition, padding_left_transition, margin_top_transition, margin_right_transition, margin_bottom_transition, margin_left_transition, border_top_left_radius_transition, border_top_right_radius_transition, border_bottom_left_radius_transition, border_bottom_right_radius_transition, border_top_width_transition, border_right_width_transition, border_bottom_width_transition, border_left_width_transition, border_top_color_transition, border_right_color_transition, border_bottom_color_transition, border_left_color_transition } = rules;
    let _: Option<Tokenized<Color>> = background;
    let _: Option<Tokenized<Color>> = color;
    let _: Option<Tokenized<Color>> = caret_color;
    let _: Option<Tokenized<Length>> = font_size;
    let _: Option<DisplayKind> = display;
    let _: Option<Vec<TrackSize>> = grid_template_columns;
    let _: Option<GridPlacement> = grid_row;
    let _: Option<GridPlacement> = grid_column;
    let _: Option<FlexDirection> = flex_direction;
    let _: Option<FlexWrap> = flex_wrap;
    let _: Option<JustifyContent> = justify_content;
    let _: Option<AlignItems> = align_items;
    let _: Option<AlignContent> = align_content;
    let _: Option<Tokenized<Length>> = gap;
    let _: Option<Tokenized<Length>> = row_gap;
    let _: Option<Tokenized<Length>> = column_gap;
    let _: Option<Tokenized<f32>> = flex_grow;
    let _: Option<Tokenized<f32>> = flex_shrink;
    let _: Option<Tokenized<Length>> = flex_basis;
    let _: Option<AlignSelf> = align_self;
    let _: Option<Tokenized<Length>> = width;
    let _: Option<Tokenized<Length>> = height;
    let _: Option<Tokenized<Length>> = min_width;
    let _: Option<Tokenized<Length>> = min_height;
    let _: Option<Tokenized<Length>> = max_width;
    let _: Option<Tokenized<Length>> = max_height;
    let _: Option<f32> = aspect_ratio;
    let _: Option<Tokenized<Length>> = padding_top;
    let _: Option<Tokenized<Length>> = padding_right;
    let _: Option<Tokenized<Length>> = padding_bottom;
    let _: Option<Tokenized<Length>> = padding_left;
    let _: Option<Tokenized<Length>> = margin_top;
    let _: Option<Tokenized<Length>> = margin_right;
    let _: Option<Tokenized<Length>> = margin_bottom;
    let _: Option<Tokenized<Length>> = margin_left;
    let _: Option<Tokenized<Length>> = border_top_left_radius;
    let _: Option<Tokenized<Length>> = border_top_right_radius;
    let _: Option<Tokenized<Length>> = border_bottom_left_radius;
    let _: Option<Tokenized<Length>> = border_bottom_right_radius;
    let _: Option<Tokenized<f32>> = border_top_width;
    let _: Option<Tokenized<f32>> = border_right_width;
    let _: Option<Tokenized<f32>> = border_bottom_width;
    let _: Option<Tokenized<f32>> = border_left_width;
    let _: Option<Tokenized<Color>> = border_top_color;
    let _: Option<Tokenized<Color>> = border_right_color;
    let _: Option<Tokenized<Color>> = border_bottom_color;
    let _: Option<Tokenized<Color>> = border_left_color;
    let _: Option<Position> = position;
    let _: Option<Tokenized<Length>> = top;
    let _: Option<Tokenized<Length>> = right;
    let _: Option<Tokenized<Length>> = bottom;
    let _: Option<Tokenized<Length>> = left;
    let _: Option<FontFamily> = font_family;
    let _: Option<FontWeight> = font_weight;
    let _: Option<FontStyle> = font_style;
    let _: Option<Tokenized<f32>> = line_height;
    let _: Option<Tokenized<f32>> = letter_spacing;
    let _: Option<TextAlign> = text_align;
    let _: Option<bool> = underline;
    let _: Option<bool> = strikethrough;
    let _: Option<TextTransform> = text_transform;
    let _: Option<Tokenized<f32>> = opacity;
    let _: Option<Overflow> = overflow;
    let _: Option<OverscrollBehavior> = overscroll_behavior;
    let _: Option<ScrollbarVisibility> = scrollbar;
    let _: Option<ObjectFit> = object_fit;
    let _: Option<Shadow> = shadow;
    let _: Option<Shadow> = text_shadow;
    let _: Option<Gradient> = background_gradient;
    let _: Option<Vec<Transform>> = transform;
    let _: Option<(Length, Length)> = transform_origin;
    let _: Option<Cursor> = cursor;
    let _: Option<UserSelect> = user_select;
    let _: Option<BorderStyle> = border_style;
    let _: Option<PointerEvents> = pointer_events;
    let _: Option<Transition> = background_transition;
    let _: Option<Transition> = color_transition;
    let _: Option<Transition> = caret_color_transition;
    let _: Option<Transition> = opacity_transition;
    let _: Option<Transition> = transform_transition;
    let _: Option<Transition> = width_transition;
    let _: Option<Transition> = height_transition;
    let _: Option<Transition> = max_width_transition;
    let _: Option<Transition> = max_height_transition;
    let _: Option<Transition> = min_width_transition;
    let _: Option<Transition> = min_height_transition;
    let _: Option<Transition> = top_transition;
    let _: Option<Transition> = right_transition;
    let _: Option<Transition> = bottom_transition;
    let _: Option<Transition> = left_transition;
    let _: Option<Transition> = padding_top_transition;
    let _: Option<Transition> = padding_right_transition;
    let _: Option<Transition> = padding_bottom_transition;
    let _: Option<Transition> = padding_left_transition;
    let _: Option<Transition> = margin_top_transition;
    let _: Option<Transition> = margin_right_transition;
    let _: Option<Transition> = margin_bottom_transition;
    let _: Option<Transition> = margin_left_transition;
    let _: Option<Transition> = border_top_left_radius_transition;
    let _: Option<Transition> = border_top_right_radius_transition;
    let _: Option<Transition> = border_bottom_left_radius_transition;
    let _: Option<Transition> = border_bottom_right_radius_transition;
    let _: Option<Transition> = border_top_width_transition;
    let _: Option<Transition> = border_right_width_transition;
    let _: Option<Transition> = border_bottom_width_transition;
    let _: Option<Transition> = border_left_width_transition;
    let _: Option<Transition> = border_top_color_transition;
    let _: Option<Transition> = border_right_color_transition;
    let _: Option<Transition> = border_bottom_color_transition;
    let _: Option<Transition> = border_left_color_transition;
}

/// The two types from the original report, used the way an app would.
#[test]
fn regression_overscroll_behavior_and_grid_placement_are_reexported() {
    let rules = StyleRules {
        overscroll_behavior: Some(OverscrollBehavior::Contain),
        grid_column: Some(GridPlacement::SPAN_ALL),
        grid_row: Some(GridPlacement::Line(2)),
        ..Default::default()
    };
    assert_eq!(rules.overscroll_behavior, Some(OverscrollBehavior::Contain));
    assert_eq!(rules.grid_column, Some(GridPlacement::Lines(1, -1)));
}

/// Types reachable only through a field's payload (a gradient's stops,
/// a transition's easing) must be nameable too, or the field can be
/// named but never filled.
#[test]
fn nested_style_payload_types_are_reexported() {
    let _ = Gradient {
        kind: GradientKind::Linear { angle_deg: 90.0 },
        stops: vec![GradientStop { offset: 0.0, color: Color("#000".into()) }],
    };
    let _ = RadialExtent::default();
    let _ = Transition::new(150, Easing::Linear);
}
