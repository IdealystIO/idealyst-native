//! Style parity across the runtime-server wire: every `StyleRules`
//! property the recorder sees reaches the client unchanged.
//!
//! The path under test is the one a wire-mode (`idealyst dev` without
//! `--local`) app takes for every style:
//! `dev_server::convert_out::style_rules_to_wire` → `wire::codec` (JSON)
//! → `dev_client::convert::wire_style_to_rules`.
//!
//! `regression_every_style_property_round_trips_the_wire` builds its
//! `StyleRules` as a literal with no `..Default::default()`, so a property
//! added to `StyleRules` fails to compile here until it is given a
//! non-default value — and then fails the equality until the wire carries
//! it. The conversions themselves are exhaustive destructures (see their
//! docs); this test is what proves each mapped value survives intact.

use runtime_shared::style::{
    AlignContent, AlignSelf, BorderStyle, Cursor, DisplayKind, FlexWrap, GridPlacement,
    OverscrollBehavior, PointerEvents, ScrollbarVisibility, Shadow, TextTransform, TrackSize,
    Transition, UserSelect,
};
use runtime_shared::{
    AlignItems, Color, Easing, FlexDirection, FontFamily, FontStyle, FontWeight, Gradient,
    GradientKind, GradientStop, JustifyContent, Length, ObjectFit, Overflow, Position,
    RadialExtent, StyleRules, TextAlign, Tokenized, Transform,
};

/// Encode on the dev side, cross the real codec, decode on the client.
fn over_the_wire(rules: &StyleRules) -> StyleRules {
    let wire_form = dev_server::convert_out::style_rules_to_wire(rules);
    let bytes = wire::codec::encode(&wire_form).expect("encode");
    let decoded: wire::WireStyleRules = wire::codec::decode(&bytes).expect("decode");
    dev_client::convert::wire_style_to_rules(decoded)
}

fn c(hex: &str) -> Option<Tokenized<Color>> {
    Some(Tokenized::Literal(Color(hex.to_string())))
}

fn px(v: f32) -> Option<Tokenized<Length>> {
    Some(Tokenized::Literal(Length::Px(v)))
}

fn n(v: f32) -> Option<Tokenized<f32>> {
    Some(Tokenized::Literal(v))
}

/// A distinct transition per property, so a transition wired to the
/// wrong field fails the equality rather than passing by coincidence.
fn t(ms: u32) -> Option<Transition> {
    let easing = match ms % 6 {
        0 => Easing::Linear,
        1 => Easing::Ease,
        2 => Easing::EaseIn,
        3 => Easing::EaseOut,
        4 => Easing::EaseInOut,
        _ => Easing::CubicBezier(0.1, 0.2, 0.3, 0.4),
    };
    Some(Transition::new(ms, easing))
}

/// Every property set, each to a non-default value distinct from its
/// neighbours'. A literal on purpose — see the module docs.
fn every_property_set() -> StyleRules {
    StyleRules {
        background: c("#010101"),
        color: c("#020202"),
        caret_color: c("#030303"),
        font_size: px(17.0),

        display: Some(DisplayKind::Grid),
        grid_template_columns: Some(vec![
            TrackSize::Auto,
            TrackSize::MinContent,
            TrackSize::MaxContent,
            TrackSize::Fr(2.0),
            TrackSize::Px(120.0),
            TrackSize::Minmax(Box::new(TrackSize::Px(80.0)), Box::new(TrackSize::Fr(1.0))),
            TrackSize::FitContent(240.0),
        ]),
        grid_row: Some(GridPlacement::Line(2)),
        grid_column: Some(GridPlacement::SPAN_ALL),

        flex_direction: Some(FlexDirection::RowReverse),
        flex_wrap: Some(FlexWrap::Wrap),
        justify_content: Some(JustifyContent::SpaceEvenly),
        align_items: Some(AlignItems::Baseline),
        align_content: Some(AlignContent::SpaceBetween),
        gap: px(4.0),
        row_gap: px(5.0),
        column_gap: px(6.0),

        flex_grow: n(1.5),
        flex_shrink: n(2.5),
        flex_basis: Some(Tokenized::Literal(Length::Percent(30.0))),
        align_self: Some(AlignSelf::FlexStart),

        width: px(100.0),
        height: Some(Tokenized::Literal(Length::Auto)),
        min_width: px(10.0),
        min_height: px(11.0),
        max_width: Some(Tokenized::Literal(Length::Percent(90.0))),
        max_height: px(13.0),
        aspect_ratio: Some(16.0 / 9.0),

        padding_top: px(21.0),
        padding_right: px(22.0),
        padding_bottom: px(23.0),
        padding_left: px(24.0),

        margin_top: px(31.0),
        margin_right: px(32.0),
        margin_bottom: px(33.0),
        margin_left: px(34.0),

        // `Full` (the pill) included: it must cross as itself, not as
        // the px sentinel.
        border_top_left_radius: Some(Tokenized::Literal(Length::Full)),
        border_top_right_radius: px(42.0),
        border_bottom_left_radius: px(43.0),
        border_bottom_right_radius: px(44.0),

        border_top_width: n(1.0),
        border_right_width: n(2.0),
        border_bottom_width: n(3.0),
        border_left_width: n(4.0),
        border_top_color: c("#0a0a0a"),
        border_right_color: c("#0b0b0b"),
        border_bottom_color: c("#0c0c0c"),
        border_left_color: c("#0d0d0d"),
        border_style: Some(BorderStyle::Dashed),

        position: Some(Position::Absolute),
        top: px(51.0),
        right: px(52.0),
        bottom: px(53.0),
        left: px(54.0),
        z_index: Some(7),

        font_family: Some(FontFamily::System("Inter, sans-serif".to_string())),
        font_weight: Some(FontWeight::SemiBold),
        font_style: Some(FontStyle::Italic),
        line_height: n(1.4),
        letter_spacing: n(0.5),
        text_align: Some(TextAlign::Justify),
        underline: Some(true),
        strikethrough: Some(true),
        text_transform: Some(TextTransform::Uppercase),
        max_lines: Some(3),

        opacity: n(0.75),
        overflow: Some(Overflow::Hidden),
        overscroll_behavior: Some(OverscrollBehavior::Contain),
        scrollbar: Some(ScrollbarVisibility::Hidden),
        object_fit: Some(ObjectFit::Cover),
        shadow: Some(Shadow { x: 1.0, y: 2.0, blur: 3.0, color: Color("#111111".into()).into() }),
        text_shadow: Some(Shadow {
            x: 4.0,
            y: 5.0,
            blur: 6.0,
            color: Color("#222222".into()).into(),
        }),
        background_gradient: Some(Gradient {
            kind: GradientKind::Radial {
                center: (0.25, 0.75),
                radius: 1.5,
                extent: RadialExtent::FarthestCorner,
            },
            stops: vec![
                GradientStop { offset: 0.0, color: Color("#333333".into()).into() },
                GradientStop { offset: 1.0, color: Color("#444444".into()).into() },
            ],
        }),
        transform: Some(vec![
            Transform::TranslateX(Length::Px(3.0)),
            Transform::TranslateY(Length::Percent(-50.0)),
            Transform::Scale(1.1),
            Transform::ScaleXY { x: 0.9, y: 1.2 },
            Transform::Rotate(45.0),
            Transform::SkewX(5.0),
            Transform::SkewY(6.0),
        ]),
        transform_origin: Some((Length::Percent(0.0), Length::Px(12.0))),

        cursor: Some(Cursor::Pointer),
        user_select: Some(UserSelect::None),
        pointer_events: Some(PointerEvents::None),

        background_transition: t(100),
        color_transition: t(101),
        caret_color_transition: t(102),
        opacity_transition: t(103),
        transform_transition: t(104),
        width_transition: t(105),
        height_transition: t(106),
        max_width_transition: t(107),
        max_height_transition: t(108),
        min_width_transition: t(109),
        min_height_transition: t(110),
        top_transition: t(111),
        right_transition: t(112),
        bottom_transition: t(113),
        left_transition: t(114),
        padding_top_transition: t(115),
        padding_right_transition: t(116),
        padding_bottom_transition: t(117),
        padding_left_transition: t(118),
        margin_top_transition: t(119),
        margin_right_transition: t(120),
        margin_bottom_transition: t(121),
        margin_left_transition: t(122),
        border_top_left_radius_transition: t(123),
        border_top_right_radius_transition: t(124),
        border_bottom_left_radius_transition: t(125),
        border_bottom_right_radius_transition: t(126),
        border_top_width_transition: t(127),
        border_right_width_transition: t(128),
        border_bottom_width_transition: t(129),
        border_left_width_transition: t(130),
        border_top_color_transition: t(131),
        border_right_color_transition: t(132),
        border_bottom_color_transition: t(133),
        border_left_color_transition: t(134),
        shadow_transition: t(135),
    }
}

/// Every `StyleRules` property survives the wire.
///
/// Regression: `WireStyleRules` mirrored only a subset, so a wire-mode
/// app silently dropped the rest — a button's `align_self: FlexStart`
/// was ignored (it stretched full width), `text_transform: Uppercase`
/// never applied, grids collapsed to flex, nothing eased. `--local` was
/// right throughout, so nothing looked like an error.
#[test]
fn regression_every_style_property_round_trips_the_wire() {
    let sent = every_property_set();
    let received = over_the_wire(&sent);
    assert_eq!(received, sent);
}

/// An empty style stays empty — nothing on the client side invents a
/// value the author did not set.
#[test]
fn an_empty_style_round_trips_empty() {
    assert_eq!(over_the_wire(&StyleRules::default()), StyleRules::default());
}

/// Unset properties cost no bytes: a style that sets one of the
/// properties added for parity carries that one key and none of its
/// unset siblings.
#[test]
fn unset_parity_properties_are_not_serialized() {
    let rules = StyleRules { align_self: Some(AlignSelf::Center), ..StyleRules::default() };
    let json = String::from_utf8(
        wire::codec::encode(&dev_server::convert_out::style_rules_to_wire(&rules)).unwrap(),
    )
    .unwrap();
    assert!(json.contains("\"align_self\":\"Center\""), "{json}");
    for absent in ["text_transform", "display", "row_gap", "shadow_transition", "user_select"] {
        assert!(!json.contains(absent), "unset `{absent}` was serialized: {json}");
    }
}

/// Every variant of every enum-valued property maps to its own twin.
/// The full-style test above exercises one variant each; this catches a
/// conversion that swaps two arms (`FlexStart` ↔ `FlexEnd`).
#[test]
fn every_enum_variant_maps_to_its_twin() {
    macro_rules! sweep {
        ($field:ident: $($v:expr),+ $(,)?) => {$(
            let rules = StyleRules { $field: Some($v), ..StyleRules::default() };
            assert_eq!(over_the_wire(&rules), rules, concat!(stringify!($field), " = ", stringify!($v)));
        )+};
    }
    sweep!(display: DisplayKind::Flex, DisplayKind::Grid);
    sweep!(flex_wrap: FlexWrap::NoWrap, FlexWrap::Wrap, FlexWrap::WrapReverse);
    sweep!(
        align_content: AlignContent::FlexStart,
        AlignContent::FlexEnd,
        AlignContent::Center,
        AlignContent::Stretch,
        AlignContent::SpaceBetween,
        AlignContent::SpaceAround,
    );
    sweep!(
        align_self: AlignSelf::Auto,
        AlignSelf::FlexStart,
        AlignSelf::FlexEnd,
        AlignSelf::Center,
        AlignSelf::Stretch,
        AlignSelf::Baseline,
    );
    sweep!(
        text_transform: TextTransform::None,
        TextTransform::Uppercase,
        TextTransform::Lowercase,
        TextTransform::Capitalize,
    );
    sweep!(font_style: FontStyle::Normal, FontStyle::Italic);
    sweep!(
        overscroll_behavior: OverscrollBehavior::Auto,
        OverscrollBehavior::Contain,
        OverscrollBehavior::None,
    );
    sweep!(scrollbar: ScrollbarVisibility::Auto, ScrollbarVisibility::Hidden);
    sweep!(user_select: UserSelect::Auto, UserSelect::None, UserSelect::Text, UserSelect::All);
    sweep!(pointer_events: PointerEvents::Auto, PointerEvents::None);
    sweep!(grid_row: GridPlacement::Line(-1), GridPlacement::Lines(1, 3));
    sweep!(underline: false, true);
    sweep!(strikethrough: false, true);
    sweep!(
        opacity_transition: Transition::new(1, Easing::Linear),
        Transition::new(2, Easing::Ease),
        Transition::new(3, Easing::EaseIn),
        Transition::new(4, Easing::EaseOut),
        Transition::new(5, Easing::EaseInOut),
        Transition::new(6, Easing::CubicBezier(0.4, 0.0, 0.2, 1.0)),
    );
}
